Name:           ibus-handy
Version:        0.7.5
Release:        8%{?dist}
Summary:        Speech-to-text for GNOME/Wayland via IBus

License:        MIT
URL:            https://github.com/rohithmahesh/Handy
Source0:        ibus-handy-%{version}.tar.gz

BuildRequires:  rustc, cargo
BuildRequires:  pkgconfig(gtk4)
BuildRequires:  pkgconfig(libadwaita-1)
BuildRequires:  pkgconfig(graphene-gobject-1.0)
BuildRequires:  pkgconfig(alsa)
BuildRequires:  pkgconfig(ibus-1.0)
BuildRequires:  pkgconfig(glib-2.0)
BuildRequires:  pkgconfig(gobject-2.0)
BuildRequires:  openssl-devel
BuildRequires:  cmake
BuildRequires:  clang-devel
BuildRequires:  glslc

Requires:       ibus >= 1.5.0
Supplements:    (gnome-shell and fedora-release-workstation)

%description
Handy provides speech-to-text dictation integrated with IBus
for GNOME on Wayland. Switch to Handy using Super+Space to
start dictating.

%prep
%autosetup -n ibus-handy-%{version}

%build
cargo build --release --features cli

%install
install -Dm755 target/release/handy %{buildroot}%{_bindir}/handy
install -Dm755 target/release/ibus-handy-engine %{buildroot}%{_libexecdir}/ibus-handy-engine

install -Dm644 packaging/fedora/handy.desktop %{buildroot}%{_datadir}/applications/com.handy.Handy.desktop
install -Dm644 resources/icons/handy.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/handy.svg
install -Dm644 resources/icons/handy.svg %{buildroot}%{_datadir}/handy/icons/handy.svg
install -Dm644 resources/models/silero_vad_v4.onnx %{buildroot}%{_datadir}/handy/models/silero_vad_v4.onnx
install -Dm644 resources/marimba_start.wav %{buildroot}%{_datadir}/handy/sounds/marimba_start.wav
install -Dm644 resources/marimba_stop.wav %{buildroot}%{_datadir}/handy/sounds/marimba_stop.wav
install -Dm644 resources/pop_start.wav %{buildroot}%{_datadir}/handy/sounds/pop_start.wav
install -Dm644 resources/pop_stop.wav %{buildroot}%{_datadir}/handy/sounds/pop_stop.wav
install -Dm644 packaging/fedora/com.handy.Transcription.service %{buildroot}%{_datadir}/dbus-1/services/com.handy.Transcription.service
install -Dm644 packaging/fedora/handy.service %{buildroot}%{_userunitdir}/handy.service
install -Dm644 packaging/fedora/handy.xml %{buildroot}%{_datadir}/ibus/component/org.freedesktop.IBus.Handy.xml
install -Dm644 data/com.handy.Transcription.gschema.xml %{buildroot}%{_datadir}/glib-2.0/schemas/com.handy.Transcription.gschema.xml

%post
ibus write-cache 2>/dev/null || :
glib-compile-schemas %{_datadir}/glib-2.0/schemas 2>/dev/null || :

%preun
%systemd_user_preun handy.service

%postun
%systemd_user_postun handy.service
ibus write-cache 2>/dev/null || :
glib-compile-schemas %{_datadir}/glib-2.0/schemas 2>/dev/null || :

%files
%doc README.md
%license LICENSE
%{_bindir}/handy
%{_libexecdir}/ibus-handy-engine
%{_datadir}/handy/icons/handy.svg
%{_datadir}/handy/models/silero_vad_v4.onnx
%{_datadir}/handy/sounds/marimba_start.wav
%{_datadir}/handy/sounds/marimba_stop.wav
%{_datadir}/handy/sounds/pop_start.wav
%{_datadir}/handy/sounds/pop_stop.wav
%{_datadir}/ibus/component/org.freedesktop.IBus.Handy.xml
%{_datadir}/applications/com.handy.Handy.desktop
%{_datadir}/icons/hicolor/scalable/apps/handy.svg
%{_datadir}/dbus-1/services/com.handy.Transcription.service
%{_datadir}/glib-2.0/schemas/com.handy.Transcription.gschema.xml
%{_userunitdir}/handy.service

%changelog
* Tue Feb 17 2026 Handy Team <handy@example.com> - 0.7.5-8
- Fix models page startup hang caused by invalid AdwPreferencesGroup child removal
- Remove unsafe periodic model row refresh loop in UI

* Tue Feb 17 2026 Handy Team <handy@example.com> - 0.7.5-7
- Align desktop file identity with GTK app ID for proper GNOME dock icon matching
- Set StartupWMClass to com.handy.Handy
- Use themed icon name for IBus engine metadata

* Tue Feb 17 2026 Handy Team <handy@example.com> - 0.7.5-6
- Fix switch row focus/activation visual regression in settings UI
- Restore native PreferencesGroup model row rendering and button spacing

* Mon Feb 16 2026 Handy Team <handy@example.com> - 0.7.5-5
- Classify Handy as a special-purpose IBus source (language=other, layout=default)
- Improve IBus engine lifecycle handling when enabling/disabling the input source
- Refresh model list UI state live and show real download progress/cancel action
- Improve preferences page layout consistency using Adwaita clamps

* Sun Feb 15 2026 Handy Team <handy@example.com> - 0.7.5-4
- Add daemon mode for D-Bus/service activation (no UI popup on auto-start)
- Install required runtime assets (Silero VAD model and feedback sounds)
- Sync runtime managers with GSettings changes
- Persist selected model and improve model selection consistency
- Make IBus stop/transcribe path non-blocking

* Sat Feb 14 2026 Handy Team <handy@example.com> - 0.7.5-3
- Fix over-constrained settings pages by removing extra clamp wrappers
- Keep full-width adaptive layout for General, Models, and Advanced pages

* Fri Feb 13 2026 Handy Team <handy@example.com> - 0.7.5-2
- Fix IBus component registration metadata
- Install IBus component to /usr/share/ibus/component/
- Improve Libadwaita preferences page layout and sidebar behavior
- Reduce redundant Cargo dependencies

* Sun Feb 16 2025 Handy Team <handy@example.com> - 0.7.5-1
- Rewritten as native GTK4/Libadwaita application
- IBus engine rewritten in Rust (no Python dependency)
- Removed all JavaScript/TypeScript frontend dependencies
- GNOME/Wayland only fork
