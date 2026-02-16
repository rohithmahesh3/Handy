Name:           handy
Version:        0.7.5
Release:        1%{?dist}
Summary:        Speech-to-text for GNOME/Wayland via IBus

License:        MIT
URL:            https://github.com/rohithmahesh/Handy
Source0:        %{url}/archive/refs/heads/fedora-gnome.tar.gz

BuildRequires:  rustc, cargo
BuildRequires:  pkgconfig(gtk+-3.0)
BuildRequires:  pkgconfig(gtk-layer-shell-0)
BuildRequires:  pkgconfig(webkit2gtk-4.1)
BuildRequires:  pkgconfig(alsa)
BuildRequires:  pkgconfig(libpipewire-0.3)
BuildRequires:  openssl-devel
BuildRequires:  python3-devel
BuildRequires:  meson

Requires:       ibus >= 1.5.0
Requires:       python3-gobject-base
Requires:       python3-dbus
Requires:       pipewire-pulseaudio
Supplements:    (gnome-shell and fedora-release-workstation)

%description
Handy provides speech-to-text dictation integrated with IBus
for GNOME on Wayland. Switch to Handy using Super+Space to
start dictating.

%prep
%autosetup -n Handy-fedora-gnome

%build
cd src-tauri
cargo build --release

cd ../ibus
%meson

%install
install -Dm755 src-tauri/target/release/handy %{buildroot}%{_bindir}/handy

cd ibus
%meson_install

install -Dm644 packaging/fedora/handy.desktop %{buildroot}%{_datadir}/applications/handy.desktop
install -Dm644 resources/icons/handy.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/handy.svg
install -Dm644 packaging/fedora/com.handy.Transcription.service %{buildroot}%{_datadir}/dbus-1/services/com.handy.Transcription.service
install -Dm644 packaging/fedora/handy.service %{buildroot}%{_userunitdir}/handy.service

%post
ibus write-cache 2>/dev/null || :

%preun
%systemd_user_preun handy.service

%postun
%systemd_user_postun handy.service
ibus write-cache 2>/dev/null || :

%files
%doc README.md
%license LICENSE
%{_bindir}/handy
%{_libexecdir}/handy-ibus-engine
%{_datadir}/handy/ibus/
%{_datadir}/ibus/components/handy.xml
%{_datadir}/applications/handy.desktop
%{_datadir}/icons/hicolor/scalable/apps/handy.svg
%{_datadir}/dbus-1/services/com.handy.Transcription.service
%{_userunitdir}/handy.service

%changelog
* Sun Feb 16 2025 Handy Team <handy@example.com> - 0.7.5-1
- Initial Fedora package with IBus integration
- GNOME/Wayland only fork
- Native IBus input method support
