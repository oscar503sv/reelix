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
use crate::capture::camera::{self as camera_capture, CameraAccess};
use crate::capture::{CaptureRequest, ScreenCastSession, SourceKind, audio as audio_capture};
use crate::config::Settings;
use crate::error::AppError;
use crate::recording::audio::{AacEncoderKind, AudioInput, AudioSourceElement};
use crate::recording::camera::{CameraInput, CameraSource};
use crate::recording::remux::Remux;
use crate::recording::{
    AudioConfig, EncoderKind, PipelineConfig, PipelineEvent, RecordingPipeline, VideoInput,
};
use crate::storage::{Library, RecordingMetadata, naming};

/// If EOS has not reached the muxer after this long, tear down anyway. The
/// MP4 stays playable thanks to mp4mux's periodically updated index.
const FINALIZE_TIMEOUT: Duration = Duration::from_secs(10);

/// Pause between hiding the countdown and starting capture: a few frames at
/// 60 Hz, enough for GNOME to repaint the bubble without the number.
const COUNTDOWN_CLEAR_DELAY: Duration = Duration::from_millis(150);

/// Callbacks into the UI.
pub trait ControllerObserver {
    fn state_changed(&self, state: &RecorderState);
    fn error(&self, error: &AppError);
    /// Non-fatal information the user should see (e.g. "recording without
    /// microphone").
    fn notice(&self, _message: &str) {}
    /// Microphone loudness, 0.0–1.0.
    fn audio_level(&self, _level: f64) {}
    /// Seconds left before capture starts (only during `Preparing`); 0 means
    /// "hide the countdown now".
    fn countdown(&self, _remaining: u32) {}
}

struct ActiveRecording {
    session: Rc<ScreenCastSession>,
    /// Keeps the camera portal's PipeWire remote open while recording.
    _camera: Option<CameraAccess>,
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
    /// The on-screen camera bubble is showing: a full-screen recording
    /// captures it, so the camera only needs compositing for window captures.
    bubble_active: Cell<bool>,
    /// Fragmented → faststart conversion running during `Processing`.
    remux: RefCell<Option<Remux>>,
}

impl Controller {
    pub fn new(settings: Settings) -> Rc<Self> {
        Rc::new(Self {
            state: RefCell::new(RecorderState::Idle),
            settings: RefCell::new(settings),
            active: RefCell::new(None),
            generation: Cell::new(0),
            observer: RefCell::new(None),
            bubble_active: Cell::new(false),
            remux: RefCell::new(None),
        })
    }

    pub fn set_observer(&self, observer: Option<Rc<dyn ControllerObserver>>) {
        *self.observer.borrow_mut() = observer;
    }

    pub fn set_bubble_active(&self, active: bool) {
        self.bubble_active.set(active);
    }

    pub fn state(&self) -> RecorderState {
        self.state.borrow().clone()
    }

    pub fn settings(&self) -> Settings {
        self.settings.borrow().clone()
    }

    /// Changes settings and persists them. Applies to the next recording.
    pub fn update_settings(&self, change: impl FnOnce(&mut Settings)) {
        let updated = {
            let mut settings = self.settings.borrow_mut();
            change(&mut settings);
            settings.clone()
        };
        if let Err(e) = updated.save() {
            tracing::error!(error = %e, "failed to save settings");
        }
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
                Ok(session) => {
                    if !this.apply(RecorderEvent::SourceSelected) {
                        session.close().await;
                        return;
                    }
                    let camera = this.request_camera(session.is_window()).await;
                    if this.generation.get() != generation {
                        tracing::debug!("cancelled while opening the camera; discarding");
                        session.close().await;
                        return;
                    }
                    this.begin_recording(Rc::new(session), camera, generation);
                }
                Err(AppError::SelectionCancelled) => {
                    tracing::info!("user cancelled source selection");
                    this.apply(RecorderEvent::SelectionCancelled);
                }
                Err(e) => this.fail(e),
            }
        });
    }

    fn countdown(self: &Rc<Self>, generation: u64, remaining: u32) {
        let still_ours =
            self.generation.get() == generation && *self.state.borrow() == RecorderState::Preparing;
        if !still_ours {
            return;
        }
        if remaining == 0 {
            // Hide the number first and give the compositor a moment to
            // repaint, otherwise the first captured frames still show "1".
            if let Some(observer) = self.observer() {
                observer.countdown(0);
            }
            let weak = Rc::downgrade(self);
            glib::timeout_add_local_once(COUNTDOWN_CLEAR_DELAY, move || {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let still_ours = this.generation.get() == generation
                    && *this.state.borrow() == RecorderState::Preparing;
                if still_ours && let Some(Err(e)) = this.with_pipeline(RecordingPipeline::start) {
                    this.fail(e);
                }
            });
            return;
        }
        if let Some(observer) = self.observer() {
            observer.countdown(remaining);
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(1), move || {
            if let Some(this) = weak.upgrade() {
                this.countdown(generation, remaining - 1);
            }
        });
    }

    /// Opens the camera for compositing when needed: camera enabled and
    /// either no bubble on screen or a single-window capture (which does not
    /// include the bubble). Any problem means recording without it (with a
    /// notice), never a failed recording.
    async fn request_camera(&self, window_capture: bool) -> Option<CameraAccess> {
        if !self.settings.borrow().camera.enabled {
            return None;
        }
        if self.bubble_active.get() && !window_capture {
            tracing::info!("camera bubble is on screen; the screen capture records it");
            return None;
        }
        match CameraAccess::request().await {
            Ok(access) => Some(access),
            Err(e) => {
                tracing::warn!(error = %e, "recording without camera");
                let (title, _) = e.user_message();
                self.notify(&format!("{title}: se graba sin cámara."));
                None
            }
        }
    }

    fn begin_recording(
        self: &Rc<Self>,
        session: Rc<ScreenCastSession>,
        camera: Option<CameraAccess>,
        generation: u64,
    ) {
        match self.build_pipeline(&session, camera) {
            Ok(active) => {
                // Built first so errors surface before the countdown; capture
                // only starts when it reaches zero, so it is never recorded.
                *self.active.borrow_mut() = Some(active);
                self.watch_session_closed(session, generation);
                let seconds = self.settings.borrow().recording.countdown_seconds;
                self.countdown(generation, seconds);
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
        camera: Option<CameraAccess>,
    ) -> Result<ActiveRecording, AppError> {
        let settings = self.settings.borrow().clone();
        let encoder = EncoderKind::detect(settings.video.encoder)?;
        let started_at = Local::now();
        let output = self.library().new_recording_path(&started_at)?;

        let audio = self.resolve_audio(&settings);
        let camera_input = camera.as_ref().map(|access| CameraInput {
            source: CameraSource::PipeWire {
                fd: access.pipewire_fd(),
                target: self.resolve_camera_device(settings.camera.device.as_deref()),
            },
            layout: settings.camera.layout(),
        });

        let config = PipelineConfig {
            video: VideoInput::PipeWire {
                fd: session.pipewire_fd(),
                node_id: session.node_id(),
            },
            audio,
            camera: camera_input,
            output: output.clone(),
            fps: settings.video.fps,
            bits_per_pixel: settings.video.quality.bits_per_pixel(),
            encoder,
            max_height: settings.video.resolution.max_height(),
        };

        let weak = Rc::downgrade(self);
        let pipeline = RecordingPipeline::new(&config, move |event| {
            if let Some(this) = weak.upgrade() {
                this.on_pipeline_event(event);
            }
        })?;

        Ok(ActiveRecording {
            session: Rc::clone(session),
            _camera: camera,
            pipeline,
            output,
            started_at,
            encoder,
            fps: settings.video.fps,
        })
    }

    /// Turns audio settings into pipeline inputs, degrading gracefully: a
    /// missing microphone or AAC encoder means recording without audio
    /// (with a notice), never a failed recording.
    fn resolve_audio(&self, settings: &Settings) -> Option<AudioConfig> {
        let mut inputs = audio_capture::plan_inputs(&settings.audio);
        if settings.audio.microphone {
            let microphones = audio_capture::list_microphones();
            tracing::debug!(?microphones, "available microphones");
            if microphones.is_empty() {
                tracing::warn!("no microphone found; recording without it");
                self.notify("No se encontró ningún micrófono: se graba sin él.");
                inputs.retain(|input| !input.meter);
            } else if let Some(wanted) = &settings.audio.microphone_device {
                if !microphones.iter().any(|m| &m.id == wanted) {
                    tracing::warn!(device = %wanted, "configured microphone missing; using default");
                    self.notify(
                        "El micrófono elegido no está conectado: se usa el predeterminado.",
                    );
                    for input in inputs.iter_mut().filter(|i| i.meter) {
                        *input = AudioInput {
                            source: AudioSourceElement::Pulse { device: None },
                            ..input.clone()
                        };
                    }
                }
            }
        }
        if inputs.is_empty() {
            return None;
        }
        match AacEncoderKind::detect() {
            Ok(encoder) => Some(AudioConfig { inputs, encoder }),
            Err(e) => {
                tracing::error!(error = %e, "no AAC encoder; recording without audio");
                self.notify("No hay codificador de audio AAC: se graba sin audio.");
                None
            }
        }
    }

    /// Returns the configured camera if it is connected, else `None`
    /// (PipeWire picks one) with a notice.
    fn resolve_camera_device(&self, wanted: Option<&str>) -> Option<String> {
        let wanted = wanted?;
        if camera_capture::list_cameras()
            .iter()
            .any(|c| c.id == wanted)
        {
            Some(wanted.to_owned())
        } else {
            tracing::warn!(device = %wanted, "configured camera missing; using default");
            self.notify("La cámara elegida no está conectada: se usa la predeterminada.");
            None
        }
    }

    fn notify(&self, message: &str) {
        if let Some(observer) = self.observer() {
            observer.notice(message);
        }
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
            PipelineEvent::AudioLevel(level) => {
                if let Some(observer) = self.observer() {
                    observer.audio_level(level);
                }
            }
            PipelineEvent::SourceLost { source } => {
                if source.starts_with(crate::recording::audio::AUDIO_SOURCE_PREFIX) {
                    self.notify("Se perdió una fuente de audio; se guardó lo grabado.");
                } else if source == crate::recording::camera::CAMERA_SOURCE_NAME {
                    self.notify("Se perdió la cámara; se guardó lo grabado.");
                }
                if self.state.borrow().is_capturing() {
                    self.apply(RecorderEvent::StopRequested);
                    self.arm_finalize_timeout();
                }
            }
            PipelineEvent::Finalized => self.finalize(),
            PipelineEvent::Error(e) => self.fail(e),
        }
    }

    /// Pause if recording, resume if paused. Bound to Ctrl+Shift+P.
    pub fn toggle_pause(self: &Rc<Self>) {
        let state = self.state();
        let (event, result) = match state {
            RecorderState::Recording => (
                RecorderEvent::PauseRequested,
                self.with_pipeline(RecordingPipeline::pause),
            ),
            RecorderState::Paused => (
                RecorderEvent::ResumeRequested,
                self.with_pipeline(RecordingPipeline::resume),
            ),
            _ => return,
        };
        match result {
            Some(Ok(())) => {
                self.apply(event);
            }
            Some(Err(e)) => self.fail(e),
            None => {}
        }
    }

    fn with_pipeline<T>(&self, f: impl FnOnce(&RecordingPipeline) -> T) -> Option<T> {
        self.active
            .borrow()
            .as_ref()
            .map(|active| f(&active.pipeline))
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
            audio: active.pipeline.has_audio(),
            camera: active.pipeline.has_camera(),
            encoder: Some(active.encoder.factory_name().to_owned()),
        };
        active.pipeline.abort();
        close_session(Rc::clone(&active.session));
        let stats = active.pipeline.stats();
        tracing::info!(
            path = %active.output.display(),
            duration,
            frames = stats.encoded,
            dropped = stats.dropped,
            avg_encode_ms = stats.avg_latency.as_secs_f64() * 1000.0,
            max_encode_ms = stats.max_latency.as_secs_f64() * 1000.0,
            "recording saved"
        );
        if stats.drop_ratio() > 0.02 {
            tracing::warn!(
                ratio = stats.drop_ratio(),
                "encoder could not keep up; frames were dropped"
            );
        }

        let metadata = Rc::new(metadata);
        let output = active.output.clone();
        let weak = Rc::downgrade(self);
        let for_remux = Rc::clone(&metadata);
        let complete = move |result: crate::error::Result<()>| {
            if let Err(e) = result {
                // The fragmented file is still a valid MP4; keep it.
                tracing::warn!(error = %e, "remux failed; keeping fragmented MP4");
            }
            if let Some(this) = weak.upgrade() {
                this.remux.borrow_mut().take();
                this.complete_recording(&output, &for_remux);
            }
        };
        match Remux::start(&active.output, complete) {
            Ok(job) => *self.remux.borrow_mut() = Some(job),
            Err(e) => {
                tracing::warn!(error = %e, "cannot start remux; keeping fragmented MP4");
                self.complete_recording(&active.output, &metadata);
            }
        }
    }

    /// Last step of a recording: metadata next to the final file.
    fn complete_recording(&self, output: &std::path::Path, metadata: &RecordingMetadata) {
        let meta_path = naming::metadata_path_for(output);
        if let Err(e) = metadata.write(&meta_path) {
            // The video itself is fine; missing metadata is not fatal.
            tracing::error!(error = %e, "failed to write metadata");
        }
        self.apply(RecorderEvent::MetadataWritten {
            path: output.to_owned(),
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
