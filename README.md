# Gaffer

A simple DMX lightboard for Linux. Gaffer controls the brightness and colour of DMX lights from a
GTK4/libadwaita window. You can save scenes and run automations: effects such as Breathe,
Rainbow, Chase, Candle, Colour cycle and Strobe, or sequences of scenes with fades. Automations
run by hand, at a time of day on chosen weekdays, or when Gaffer opens. A built-in soundboard can
play sound files and drive the lights from them.

It is aimed at small setups: a few LED PARs or strips on one DMX universe (512 channels) driven
from a laptop through a cheap USB-DMX interface. It is not a full lighting console.

> No screenshots yet.

## Features

- **Lights**: brightness, colour, on/off and *Identify* (flash) per light, plus a master fader and
  blackout. Built-in light types are RGB, Dimmer + RGB, RGBW, Dimmer + RGBW, RGBA,
  "LED PAR 7 ch", "LED PAR 8 ch" and Dimmer. For other lights, type the channel order as letters
  (`d` dimmer, `r` `g` `b` `w` `a` amber, `u` UV, `0` always off, `f` always full).
- **Scenes**: *Save current look*, then tap a scene to fade to it.
- **Automations**: effects or step sequences (fade + hold, optional repeat), started by hand, at a
  daily time on chosen weekdays, or when Gaffer starts.
- **Simulator**: without an interface, the stage on the Lights page shows what the lights would do.
  *Try sample lights* loads four lights, three scenes and two automations.
- **Soundboard**: each sound file in a folder becomes a pad. Each sound can carry a light cue.
- **Remote control**: `gaffer ctl <action>` sends commands to the running window through a FIFO.
- **Touch Bar (optional)**: scenes for the *barkeep* Touch Bar tool (see below).

## Requirements

- **Linux.** The DMX output uses Linux-only interfaces (sysfs, `termios2` ioctls, usbdevfs).
- **Rust** with edition 2024 support (Rust 1.85 or newer) and Cargo.
- **GTK 4.14+**, **libadwaita 1.5+** and **GStreamer 1.x** with the *base* and *good* plugin sets,
  plus their development headers and `pkg-config`. On Debian/Ubuntu/Mint, for example:

      sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
          libgstreamer1.0-dev gstreamer1.0-plugins-base gstreamer1.0-plugins-good

  (Ubuntu 24.04 / Linux Mint 22 or newer ship recent enough GTK and libadwaita.)

### DMX hardware

- A USB-DMX interface. Gaffer supports three kinds:
  - **Enttec DMX USB Pro protocol** ("smart" boxes that handle the DMX timing themselves: Enttec
    DMX USB Pro, DMXking ultraDMX and compatibles). These are the most reliable.
  - **Open DMX**: the cheapest "USB to DMX512" cables, a plain USB-serial chip (FTDI FT232R,
    WCH CH340/CH9102, Silicon Labs CP210x or Prolific PL2303) plus an RS-485 driver. The computer
    does the DMX timing. Example: DSD TECH SH-RS09B (or SH-RS09C for USB-C).
  - **uDMX**: Anyma uDMX dongles and clones (USB `16c0:05dc`).

  The type is detected automatically where possible. If a cheap cable misbehaves, set *Protocol*
  in Setup by hand.
- A USB cable that fits the interface, DMX (XLR) cables (3-pin on most LED lights), and a 120 Ω
  terminator on the last light in the chain.
- Your user must be in the `dialout` group to open USB-serial ports. uDMX dongles need the udev
  rule in `dist/60-gaffer-udmx.rules`, which `install.sh` installs.

## Install

```sh
git clone https://github.com/Luna-Nyro/gaffer.git
cd gaffer
./install.sh
```

`install.sh` installs for the current user and does not need root for the main steps. It:

1. runs `cargo build --release -p gaffer`;
2. installs the binary to `~/.local/bin/gaffer`, plus a menu entry and icon under
   `~/.local/share/`;
3. tries to install the uDMX udev rule to `/etc/udev/rules.d/` with `sudo -n` (no password
   prompt). If that isn't possible, it prints the command to run by hand;
4. adds the Touch Bar scenes if barkeep is set up (skipped otherwise);
5. reminds you to join the `dialout` group if you aren't in it:
   `sudo usermod -aG dialout $USER`, then log out and back in.

Make sure `~/.local/bin` is on your `PATH`.

## Usage

Start **Gaffer** from the menu, or run `gaffer`.

1. **Setup** → choose the interface → **Add light** (type + start address). The new light flashes
   so you can check it. Chain the lights interface → light 1 → light 2 …, put each light in DMX
   mode, and give each one a start address that doesn't overlap the others. Gaffer suggests the
   next free one.
2. **Lights**: brightness, colour, on/off, Identify.
3. **Scenes**: *Save current look*, tap to fade to it.
4. **Automations**: *New effect* or *New sequence*, then *Try it*.

The show is saved in `~/.config/gaffer/show.json` (or `$XDG_CONFIG_HOME/gaffer/show.json`).
Quitting leaves the lights as they are, like a console does. If the saved file can't be read,
Gaffer keeps it aside as `show.broken-<time>.json` and starts empty.

### Soundboard

The ♪ button (Ctrl+K) opens the soundboard beside the lights. In a narrow window it slides over
them. Every sound file in its folder becomes a pad: tap to play, tap again to stop, and several
can play at once. The folder is `~/Music/Gaffer Sounds` unless you pick another in the ⋮ menu.
MP3, WAV, OGG, FLAC, M4A, Opus and anything else GStreamer can decode are supported. Add sounds
with +, by dropping files on the panel, or by copying them into the folder. New files show up by
themselves. Right-click a pad (or open its ⋮ menu) to:

- rename it (the button only, not the file);
- set its volume;
- make it repeat;
- move it;
- move the file to the Trash.

Each sound can have a light cue: **pulse with the sound** (the lights follow its loudness, in one
colour or their own), **flash**, **a scene**, **an automation** or **blackout**. With *Put the
lights back when it ends*, the lights return to how they were when the sound ends or is stopped,
unless you changed them by hand in the meantime. The latest sound's cue wins. Sound plays through
GStreamer on the system's default output.

### Keyboard shortcuts

| Keys | Action |
| --- | --- |
| Ctrl+1 … Ctrl+4 | Lights / Scenes / Automations / Setup page |
| Ctrl+B | Blackout |
| Ctrl+Up / Ctrl+Down | Master up / down |
| Alt+Right / Alt+Left | Next / previous scene |
| Ctrl+S | Save scene |
| Ctrl+. | Stop automation |
| Ctrl+N | Add light |
| Ctrl+K | Show / hide soundboard |
| F1 … F12 | Play sounds 1–12 |
| Shift+Esc | Stop all sounds |

### Command line

```text
gaffer                       open the window
gaffer ctl <action>          control the open window
gaffer --list-ports          show USB serial ports that might be DMX interfaces
gaffer --install-bar-scenes  add Gaffer's Touch Bar scenes (barkeep)
gaffer --remove-bar-scenes   take them out again
gaffer --help | --version
```

`gaffer ctl` actions: `blackout`, `blackout-on`, `blackout-off`, `master-up`, `master-down`,
`scene-1` … `scene-12`, `scene-next`, `scene-prev`, `all-off`, `play-1` … `play-12`,
`play-next`, `play-prev`, `play-toggle`, `stop`, `sound-1` … `sound-12`, `sounds-stop`,
`sounds` (show/hide the soundboard), `show`, `page-lights`, `page-scenes`,
`page-automations`, `page-setup`.

The running app reads one action per line from the FIFO `$XDG_RUNTIME_DIR/gaffer.ctl` (mode
0600). Only names from this fixed list are accepted, and nothing is executed. `gaffer ctl show`
starts Gaffer if it isn't running.

### Touch Bar (optional)

If the barkeep Touch Bar tool is configured (`~/.config/barkeep/` exists), Gaffer can add its
own scenes to barkeep's `scenes.json`. barkeep is a separate program and is not included here.

- **Pages**: four scenes follow the window's page and include Esc, Blackout, master −/+, your
  first scenes or automations by name, next/previous and page keys.
- **Sounds page**: a ♫ key opens it, with stop-all and your first 8 sounds.

Every key runs `gaffer ctl …`. Before the first change, Gaffer backs up `scenes.json` as
`scenes.json.bak-before-gaffer`. While the scenes are installed, Gaffer keeps their names up to
date. To turn them on or off, use Setup → Touch Bar, or `gaffer --install-bar-scenes` /
`--remove-bar-scenes`. Gaffer works the same without barkeep.

## Uninstall

```sh
./install.sh --remove
```

This removes the Touch Bar scenes, the binary, the menu entry and the icon. It also removes the
uDMX udev rule if `sudo -n` is allowed; otherwise it tells you to delete
`/etc/udev/rules.d/60-gaffer-udmx.rules` by hand.

Your data is kept. Delete it yourself if you want:

- `~/.config/gaffer/` (the show);
- `~/Music/Gaffer Sounds/` (your sound files);
- `~/.config/barkeep/scenes.json.bak-before-gaffer` (if you used the Touch Bar).

## Troubleshooting

The *Help* section on the Setup page has the same checklist.

1. **Status doesn't say "Sending to …"**: if it reports no permission, join the `dialout` group
   and log in again. For a uDMX dongle, run `install.sh` once (it adds the udev rule), then
   replug the dongle.
2. **Nothing lights up**: check that Blackout is off and Master is up.
3. **Wrong mode or address**: the light must be in DMX mode (not auto, sound or master/slave)
   with the address entered in Gaffer.
4. **Wrong light flashes**: press *Identify*. If a different light flashes, the addresses are
   swapped.
5. **Colours are mixed up**: the channel order is different. Edit the light and pick another
   type, or use *Custom*.
6. **Flicker on long chains**: add a 120 Ω terminator to the last light.
7. **A cheap cable flickers or does nothing**: set *Protocol* to "Open DMX" by hand. A few cheap
   cables are Enttec Pro clones, so try that too.
8. **Is the interface seen at all?** `gaffer --list-ports` lists USB serial devices and what
   Gaffer thinks they are.
9. **No sound**: make sure the GStreamer *base* and *good* plugins are installed.

## Building and testing

```sh
cargo build --release -p gaffer
cargo test --workspace
```

The tests cover the engine, effects, sound cues, schedules, DMX encoding, a pty loopback of the
serial output, and GStreamer playback and level metering into a fakesink.

Layout:

- `core/`: the pure engine (fixtures, looks, scenes, effects, schedules, sound cues). It has no
  I/O and no GTK.
- `output/`: port discovery and the output thread for Enttec Pro, Open DMX and uDMX, which sends
  about 40 frames per second and reconnects when the interface is replugged.
- `app/`: the GTK4/libadwaita window, the `gaffer ctl` FIFO and barkeep scenes (`bar.rs`), and
  the soundboard (`audio.rs` plays, `soundboard.rs` is the sidebar).
- `dist/`: desktop entry, icon, uDMX udev rule.

## Status and limitations

Gaffer is an early personal project (version 0.1.0). It works for the author's small rig but has
had little testing beyond that.

- Linux only.
- One DMX universe and one interface. No Art-Net or sACN.
- Fixtures are dimmer/colour channels only: no moving heads, pan/tilt or gobos. Extra channels
  can only be held at 0 or 255.
- Open DMX timing is done in software and depends on the USB-serial chip and driver, so the
  cheapest cables can be less steady than an Enttec Pro-type interface.
- The interface and messages are in English only.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## Credits

Written by Luna-Nyro, with help from Claude (Anthropic).

Gaffer is built on [gtk4-rs, libadwaita-rs and gstreamer-rs](https://gtk-rs.org/) (MIT /
Apache-2.0), [serde](https://serde.rs/), `serde_json`, `libc` and `async-channel`. It runs on
GTK, libadwaita and GStreamer (LGPL-2.1-or-later). The DMX protocols come from public
descriptions of the Enttec DMX USB Pro API and the Anyma uDMX USB interface.

## License

Gaffer is free software: you can redistribute it and/or modify it under the terms of the GNU General
Public License as published by the Free Software Foundation, either version 3 of the License, or (at
your option) any later version. See [LICENSE](LICENSE).
