Name:           ibus-handy
Version:        0.7.5
Release:        1%{?dist}
Summary:        Speech-to-text for GNOME/Wayland via IBus

License:        MIT
URL:            https://github.com/rohithmahesh/Handy
Source0:        ibus-handy-%{version}.tar.gz

BuildRequires:  rustc, cargo
BuildRequires:  pkgconfig(gtk4)
BuildRequires:  pkgconfig(libadwaita-1)
BuildRequires:  pkgconfig(graphene-gobject-1.0)
BuildRequires:  pkgconfig(alsa)
BuildRequires:  pkgconfig(libpipewire-0.3)
BuildRequires:  pkgconfig(libevdev)
BuildRequires:  pkgconfig(ibus-1.0)
BuildRequires:  pkgconfig(glib-2.0)
BuildRequires:  pkgconfig(gobject-2.0)
BuildRequires:  openssl-devel
BuildRequires:  cmake
BuildRequires:  clang-devel
BuildRequires:  glslc

Requires:       ibus >= 1.5.0
Requires:       pipewire-pulse
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

install -Dm644 packaging/fedora/handy.desktop %{buildroot}%{_datadir}/applications/handy.desktop
install -Dm644 resources/icons/handy.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/handy.svg
install -Dm644 resources/icons/handy.svg %{buildroot}%{_datadir}/handy/icons/handy.svg
install -Dm644 packaging/fedora/com.handy.Transcription.service %{buildroot}%{_datadir}/dbus-1/services/com.handy.Transcription.service
install -Dm644 packaging/fedora/handy.service %{buildroot}%{_userunitdir}/handy.service
install -Dm644 packaging/fedora/handy.xml %{buildroot}%{_datadir}/ibus/components/handy.xml
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
%{_datadir}/ibus/components/handy.xml
%{_datadir}/applications/handy.desktop
%{_datadir}/icons/hicolor/scalable/apps/handy.svg
%{_datadir}/dbus-1/services/com.handy.Transcription.service
%{_datadir}/glib-2.0/schemas/com.handy.Transcription.gschema.xml
%{_userunitdir}/handy.service

%changelog
* Sun Feb 16 2025 Handy Team <handy@example.com> - 0.7.5-1
- Rewritten as native GTK4/Libadwaita application
- IBus engine rewritten in Rust (no Python dependency)
- Removed all JavaScript/TypeScript frontend dependencies
- GNOME/Wayland only fork
