//! Coordinates the portal, the GStreamer pipeline, storage and the state
//! machine. Runs on the GTK main thread; heavy work happens in GStreamer's
//! own streaming threads, so nothing here blocks the UI.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use chrono::{DateTime, Local};
use gtk::glib;

use super::state::{RecorderEvent, RecorderState};
use crate::capture::{CaptureRequest, ScreenCastSession, SourceKind};
use crate::config::Settings;
use crate::error::AppError;
use crate::recording::{EncoderKind, PipelineConfig, PipelineEvent, RecordingPipeline};
use crate::storage::{Library, RecordingMetadata, naming};

/// If EOS has not reached the muxer after this long, tear down anyway. The
/// MP4 stays playable thanks to mp4mux's periodically updated index.
const FINALIZE_TIMEOUT: Duration = Duration::from_secs(10);

/// Callbacks into the UI.
pub trait ControllerObserver {
    fn state_changed(&self, state: &RecorderState);
    fn error(&self, error: &AppError);
}

struct ActiveRecording {
    session: Rc<ScreenCastSession>,
    pipeline: RecordingPipeline,
    output: PathBuf,
    started_at: DateTime<Local>,
    encoder: EncoderKind,
    fps: u32,
}

pub struct Controller {
    state: RefCell<RecorderState>,
    settings: RefCell<Settings>,
    active: RefCell<Option<ActiveRecording>>,
    /// Bumped on every new attempt so stale async tasks can detect that the
    /// user cancelled or started over while they were awaiting.
    generation: Cell<u64>,
    observer: RefCell<Option<Rc<dyn ControllerObserver>>>,
}

impl Controller {
    pub fn new(settings: Settings) -> Rc<Self> {
        Rc::new(Self {
            state: RefCell::new(RecorderState::Idle),
            settings: RefCell::new(settings),
            active: RefCell::new(None),
            generation: Cell::new(0),
            observer: RefCell::new(None),
        })
    }

    pub fn set_observer(&self, observer: Option<Rc<dyn ControllerObserver>>) {
        *self.observer.borrow_mut() = observer;
    }

    pub fn state(&self) -> RecorderState {
        self.state.borrow().clone()
    }

    pub fn library(&self) -> Library {
        Library::new(self.settings.borrow().recording_dir())
    }

    /// Elapsed recording time, from the pipeline clock.
    pub fn elapsed(&self) -> Option<Duration> {
        self.active
            .borrow()
            .as_ref()
            .and_then(|a| a.pipeline.position())
    }

    /// Start if idle, stop if recording. Bound to Ctrl+Shift+R.
    pub fn toggle(self: &Rc<Self>, parent: Option<gtk::Window>) {
        if self.state.borrow().is_capturing() {
            self.stop();
        } else if !self.state.borrow().is_busy() {
            self.start(parent);
        }
    }

    pub fn start(self: &Rc<Self>, parent: Option<gtk::Window>) {
        if !self.apply(RecorderEvent::StartRequested) {
            return;
        }
        let generation = self.generation.get() + 1;
        self.generation.set(generation);

        let this = Rc::clone(self);
        glib::spawn_future_local(async move {
            let identifier = match &parent {
                Some(window) => ashpd::WindowIdentifier::from_native(window).await,
                None => None,
            };
            let request = CaptureRequest {
                kind: SourceKind::MonitorOrWindow,
                show_cursor: this.settings.borrow().video.show_cursor,
            };
            let result = ScreenCastSession::request(request, identifier.as_ref()).await;

            if this.generation.get() != generation
                || *this.state.borrow() != RecorderState::SelectingSource
            {
                tracing::debug!("source selection finished after cancel; discarding");
                if let Ok(session) = result {
                    session.close().await;
                }
                return;
            }

            match result {
                Ok(session) => this.begin_recording(Rc::new(session), generation),
                Err(AppError::SelectionCancelled) => {
                    tracing::info!("user cancelled source selection");
                    this.apply(RecorderEvent::SelectionCancelled);
                }
                Err(e) => this.fail(e),
            }
        });
    }

    fn begin_recording(self: &Rc<Self>, session: Rc<ScreenCastSession>, generation: u64) {
        if !self.apply(RecorderEvent::SourceSelected) {
            return;
        }
        match self.build_pipeline(&session) {
            Ok(active) => {
                if let Err(e) = active.pipeline.start() {
                    close_session(session);
                    self.fail(e);
                    return;
                }
                *self.active.borrow_mut() = Some(active);
                self.watch_session_closed(session, generation);
            }
            Err(e) => {
                close_session(session);
                self.fail(e);
            }
        }
    }

    fn build_pipeline(
        self: &Rc<Self>,
        session: &Rc<ScreenCastSession>,
    ) -> Result<ActiveRecording, AppError> {
        let settings = self.settings.borrow().clone();
        let encoder = EncoderKind::detect(settings.video.encoder)?;
        let started_at = Local::now();
        let output = self.library().new_recording_path(&started_at)?;

        let config = PipelineConfig {
            pipewire_fd: session.pipewire_fd(),
            node_id: session.node_id(),
            output: output.clone(),
            fps: settings.video.fps,
            bits_per_pixel: settings.video.quality.bits_per_pixel(),
            encoder,
        };

        let weak = Rc::downgrade(self);
        let pipeline = RecordingPipeline::new(&config, move |event| {
            if let Some(this) = weak.upgrade() {
                this.on_pipeline_event(event);
            }
        })?;

        Ok(ActiveRecording {
            session: Rc::clone(session),
            pipeline,
            output,
            started_at,
            encoder,
            fps: settings.video.fps,
        })
    }

    /// Stopping from GNOME's screen-sharing indicator closes the portal
    /// session; treat that as a normal stop.
    fn watch_session_closed(self: &Rc<Self>, session: Rc<ScreenCastSession>, generation: u64) {
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            session.closed().await;
            if let Some(this) = weak.upgrade() {
                if this.generation.get() == generation && this.state.borrow().is_capturing() {
                    tracing::info!("portal session closed by the compositor; stopping");
                    this.stop();
                }
            }
        });
    }

    fn on_pipeline_event(self: &Rc<Self>, event: PipelineEvent) {
        match event {
            PipelineEvent::Started => {
                self.apply(RecorderEvent::PipelineStarted);
            }
            PipelineEvent::SourceLost => {
                if self.state.borrow().is_capturing() {
                    self.apply(RecorderEvent::StopRequested);
                    self.arm_finalize_timeout();
                }
            }
            PipelineEvent::Finalized => self.finalize(),
            PipelineEvent::Error(e) => self.fail(e),
        }
    }

    pub fn stop(self: &Rc<Self>) {
        if !self.state.borrow().is_capturing() || !self.apply(RecorderEvent::StopRequested) {
            return;
        }
        if let Some(active) = self.active.borrow().as_ref() {
            active.pipeline.finish();
        }
        self.arm_finalize_timeout();
    }

    fn arm_finalize_timeout(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let generation = self.generation.get();
        glib::timeout_add_local_once(FINALIZE_TIMEOUT, move || {
            if let Some(this) = weak.upgrade() {
                if this.generation.get() == generation
                    && *this.state.borrow() == RecorderState::Stopping
                {
                    tracing::warn!("EOS did not arrive in time; forcing teardown");
                    this.finalize();
                }
            }
        });
    }

    /// EOS reached the sink (or timed out): write metadata and clean up.
    fn finalize(self: &Rc<Self>) {
        if !self.apply(RecorderEvent::Finalized) {
            return;
        }
        let Some(active) = self.active.borrow_mut().take() else {
            self.fail(AppError::Pipeline(
                "finalized without an active recording".into(),
            ));
            return;
        };

        let duration = active
            .pipeline
            .position()
            .map(|d| d.as_secs_f64())
            .unwrap_or_else(|| (Local::now() - active.started_at).as_seconds_f64());
        let metadata = RecordingMetadata {
            title: format!("Grabación {}", active.started_at.format("%d/%m/%Y %H:%M")),
            created_at: active.started_at,
            duration,
            resolution: active
                .pipeline
                .resolution()
                .map(|(w, h)| format!("{w}x{h}")),
            fps: active.fps,
            audio: false,
            camera: false,
            encoder: Some(active.encoder.factory_name().to_owned()),
        };
        active.pipeline.abort();
        close_session(Rc::clone(&active.session));

        let meta_path = naming::metadata_path_for(&active.output);
        if let Err(e) = metadata.write(&meta_path) {
            // The video itself is fine; missing metadata is not fatal.
            tracing::error!(error = %e, "failed to write metadata");
        }
        tracing::info!(path = %active.output.display(), duration, "recording saved");
        self.apply(RecorderEvent::MetadataWritten {
            path: active.output,
        });
    }

    /// Esc: abandon whatever is in progress. A cancelled recording is deleted.
    pub fn cancel(&self) {
        let state = self.state();
        if !state.is_busy() || matches!(state, RecorderState::Stopping | RecorderState::Processing)
        {
            return;
        }
        self.generation.set(self.generation.get() + 1);
        if let Some(active) = self.active.borrow_mut().take() {
            active.pipeline.abort();
            close_session(active.session);
            for path in [&active.output, &naming::metadata_path_for(&active.output)] {
                if let Err(e) = std::fs::remove_file(path) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!(path = %path.display(), error = %e, "failed to delete cancelled file");
                    }
                }
            }
            tracing::info!("recording cancelled and discarded");
        }
        self.apply(RecorderEvent::Cancel);
    }

    /// Acknowledge Completed/Failed and return to Idle.
    pub fn reset(&self) {
        if matches!(
            *self.state.borrow(),
            RecorderState::Completed { .. } | RecorderState::Failed { .. }
        ) {
            self.apply(RecorderEvent::Reset);
        }
    }

    fn fail(&self, error: AppError) {
        tracing::error!(error = %error, state = %self.state.borrow(), "recording failed");
        self.generation.set(self.generation.get() + 1);
        if let Some(active) = self.active.borrow_mut().take() {
            active.pipeline.abort();
            close_session(active.session);
            // Keep the partial file: mp4mux's reserved index keeps it playable.
            tracing::warn!(path = %active.output.display(), "partial recording kept");
        }
        if self.state.borrow().is_busy() {
            self.apply(RecorderEvent::Error {
                reason: error.to_string(),
            });
        }
        if let Some(observer) = self.observer() {
            observer.error(&error);
        }
    }

    /// Applies an event to the state machine and notifies the UI. Returns
    /// whether the transition was valid.
    fn apply(&self, event: RecorderEvent) -> bool {
        let next = self.state.borrow().transition(&event);
        match next {
            Ok(next) => {
                tracing::debug!(from = %self.state.borrow(), to = %next, ?event, "state transition");
                *self.state.borrow_mut() = next.clone();
                if let Some(observer) = self.observer() {
                    observer.state_changed(&next);
                }
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "ignored event");
                false
            }
        }
    }

    fn observer(&self) -> Option<Rc<dyn ControllerObserver>> {
        self.observer.borrow().clone()
    }
}

fn close_session(session: Rc<ScreenCastSession>) {
    glib::spawn_future_local(async move { session.close().await });
}
