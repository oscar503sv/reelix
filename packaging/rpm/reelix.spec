# Reelix RPM. Builds offline from two sources made by build-rpm.sh:
#   Source0: git archive of the tagged tree
#   Source1: `cargo vendor` of all Rust dependencies
%bcond check 1
# Application, not a library crate: don't ship its sources.
%global cargo_install_lib 0

Name:           reelix
Version:        0.7.1
Release:        1%{?dist}
Summary:        Screen recorder for GNOME with microphone, system audio and camera bubble

# Reelix itself is MIT. The rest is the statically linked Rust
# dependencies, as reported by %%cargo_license_summary.
SourceLicense:  MIT
License:        MIT AND MPL-2.0 AND Unicode-3.0 AND (MIT OR Apache-2.0) AND (Unlicense OR MIT) AND (MIT OR Apache-2.0 OR LGPL-2.1-or-later) AND (Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT)
URL:            https://github.com/oscar503sv/reelix
Source0:        %{url}/archive/v%{version}/%{name}-%{version}.tar.gz
# Made by build-rpm.sh (`cargo vendor`); attach it to the GitHub release.
Source1:        %{name}-%{version}-vendor.tar.xz

BuildRequires:  cargo-rpm-macros >= 26
BuildRequires:  pkgconfig(gtk4) >= 4.18
BuildRequires:  pkgconfig(libadwaita-1) >= 1.8
BuildRequires:  pkgconfig(gstreamer-1.0)
BuildRequires:  pkgconfig(gstreamer-base-1.0)
BuildRequires:  pkgconfig(gstreamer-video-1.0)
BuildRequires:  desktop-file-utils
BuildRequires:  /usr/bin/appstreamcli
%if %{with check}
# Integration tests run real GStreamer pipelines.
BuildRequires:  gstreamer1-plugins-base
BuildRequires:  gstreamer1-plugins-good
BuildRequires:  gstreamer1-plugins-bad-free
%endif

# Capture: XDG portals (ScreenCast, Camera) and PipeWire.
Requires:       xdg-desktop-portal
Recommends:     xdg-desktop-portal-gnome
Requires:       pipewire-gstreamer
Requires:       gstreamer1-plugin-gtk4
Requires:       gstreamer1-plugins-base
# pulsesrc, mp4mux/qtdemux, level, audiodynamic, aspectratiocrop
Requires:       gstreamer1-plugins-good
# h264parse, fdkaacenc and the VA-API elements (GPU encoding)
Requires:       gstreamer1-plugins-bad-free
# An H.264 encoder is needed when there is no VA-API one: openh264
# (Cisco repo, enabled by default on Fedora) or x264 (RPM Fusion).
Recommends:     gstreamer1-plugin-openh264
Requires:       hicolor-icon-theme

%description
Reelix records your screen or a window together with your microphone,
system audio and webcam into an MP4 file (H.264 + AAC), Loom style: an
on-screen camera bubble you can drag around, pause, countdown, and a
library of your recordings. It runs entirely on your computer, uses the
XDG portals on Wayland and encodes on the GPU when possible.

%prep
%autosetup -n %{name}-%{version} -a1
%cargo_prep -v vendor

%build
%cargo_build
%{cargo_license_summary}
%{cargo_license} > LICENSE.dependencies
%{cargo_vendor_manifest}

%install
%cargo_install
install -Dpm0644 data/io.github.oscar503sv.Reelix.desktop \
    %{buildroot}%{_datadir}/applications/io.github.oscar503sv.Reelix.desktop
install -Dpm0644 data/io.github.oscar503sv.Reelix.metainfo.xml \
    %{buildroot}%{_metainfodir}/io.github.oscar503sv.Reelix.metainfo.xml
cp -pr data/icons %{buildroot}%{_datadir}/

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/io.github.oscar503sv.Reelix.desktop
appstreamcli validate --no-net %{buildroot}%{_metainfodir}/io.github.oscar503sv.Reelix.metainfo.xml
%if %{with check}
# Pipelines need hardware-independent elements only; tests that need a
# display, camera or GPU skip themselves.
%cargo_test
%endif

%files
%license LICENSE LICENSE.dependencies cargo-vendor.txt
%doc README.md ARCHITECTURE.md
%{_bindir}/reelix
%{_datadir}/applications/io.github.oscar503sv.Reelix.desktop
%{_metainfodir}/io.github.oscar503sv.Reelix.metainfo.xml
%{_datadir}/icons/hicolor/*/apps/io.github.oscar503sv.Reelix*

%changelog
* Sun Nov 01 2026 Oscar Aragon <oscararagon.hdz@gmail.com> - 0.7.1-1
- App icon in the main window, screenshots in the metainfo

* Sun Nov 01 2026 Oscar Aragon <oscararagon.hdz@gmail.com> - 0.7.0-1
- First RPM release
