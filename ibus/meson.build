project('handy-ibus',
  version: '0.7.5',
  meson_version: '>= 0.59.0',
  license: 'MIT',
)

prefix = get_option('prefix')
datadir = prefix / get_option('datadir')
libexecdir = prefix / get_option('libexecdir')
ibusdir = datadir / 'ibus' / 'components'

python = find_program('python3')

install_data(
  'handy_ibus/__init__.py',
  'handy_ibus/dbus_client.py',
  'handy_ibus/engine.py',
  'handy_ibus/main.py',
  install_dir: datadir / 'handy' / 'ibus',
)

config = configuration_data()
config.set('VERSION', meson.project_version())
config.set('PYTHON', python.full_path())

configure_file(
  input: 'handy.xml.in',
  output: 'handy.xml',
  configuration: config,
  install_dir: ibusdir,
)

configure_file(
  input: 'engine.in',
  output: 'handy-ibus-engine',
  configuration: config,
  install_dir: libexecdir,
  install_mode: 'rwxr-xr-x',
)
