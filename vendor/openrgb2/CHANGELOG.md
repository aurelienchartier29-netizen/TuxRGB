# Changelog

This project uses [semantic versioning](https://semver.org/).

One caveat is that this project is still young, so some smaller breaking changes can be expected between updates, especially in newer API's.

<!--
  ### Added
  ### Changed
  ### Deprecated
  ### Removed
  ### Fixed
  ### Security
 -->

## [0.4.1]

### Changed

- Improved performance of `CommandGroup::execute()`.


## [0.4.0] - Update to SDK version 6

### Added
- Support for OpenRGB 1.0 with SDK version 6

## [0.3.0]

### Added

- `ControllerMode` and `ControllerModeBuilder` to access and configure controller modes (#14).
- Added `Led` struct and API to access controller LEDs.
- [internal] Added (basic) plugin API
  - Added OpenRGBEffects plugin support
- [internal] Added more lints

### Changed

- Breaking: changed `Segment::segment_id() -> Segment::id()`
- Breaking: change `Zone::get_all_segments() -> Zone::segment_iter()`


## Fixed

- `Controller::init()` sets the brightness to max value. Some controllers were initialised with a brightness of 0 before.

## [0.2.1]

### Added

- Addded more documentation

### Changed

- Use `Into<Color>` for methods that used to take a `Color`
- Remove `async_trait` dependency

## [0.2.0]

### Changed

- Breaking: change `Command` struct and method names to be more consistent.
- Breaking: replaced `data()` method by several methods that return that data instead.
  - ie `Controller::data().name()` -> `Controller::name()`
- Most methods now take a `impl Iterator<Item = Color>`

### Fixed

- Fix some broken links in docs

## [0.1.0]

### Changed

- Changed the way parsing works. In short, the entire message is read to a buffer and then parsed, as opposed to being read byte-by-byte from stream.

## [0.1.0]

### Changed

- Changed the way parsing works. In short, the entire message is read to a buffer and then parsed, as opposed to being read byte-by-byte from stream.

Initial version, see the [original repo](https://github.com/nicoulaj/openrgb-rs) for the full history.

## TuxRGB vendored patch (on top of 0.4.1)

- Flags: unknown bits are ignored (`FlagSet::new_truncated`) instead of failing
  the session; OpenRGB 1.0 final sends new controller/zone flags.
- `DeviceType`: added OpenRGB 1.0 types (Storage = 14 … Monitor = 20,
  Unknown = 21); unknown values map to `Unknown`.
- `Controller::set_controllable_mode`: picks the mode the way OpenRGB does
  (`Direct`, then `Custom`, then `Static`, per-LED or mode-specific colours)
  and sends it once with the maximum hardware brightness. Upstream sent the
  max-brightness mode and then the original mode again, which restored a low
  hardware brightness.
- Added `Controller::controllable_mode`, `Controller::apply_mode` and
  `ModeData::set_colors` (single-colour devices driven through their mode
  colour).
- Packets announcing more than 64 MiB are rejected instead of being
  allocated (a desynchronised stream could ask for up to 4 GiB).
- Server notifications that nobody waits for (one `RGBControllerSignalUpdate`
  per controller and frame on SDK v6) are discarded through a reused buffer
  instead of one allocation each. `DeviceListUpdated` and controller layout
  changes (zones, segments, hidden devices) set a flag read with
  `OpenRgbClient::take_device_list_updated`.
- Added `OpenRgbClient::get_controller_ids` (SDK v6, cheap list probe).
- `rescan_devices` only waits for the server to accept the request (upstream
  failed on the acknowledgement and printed to stdout).
- `get_controller_count` returns an error instead of panicking on SDK v6.
- `TCP_NODELAY` on the SDK socket (small LED packets leave at once).
