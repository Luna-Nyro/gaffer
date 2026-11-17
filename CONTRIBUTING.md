# Contributing to Gaffer

Thanks for your interest! Gaffer is a small personal project, so please keep changes focused.

## Reporting problems

Open an issue and include:

- your distribution and its GTK, libadwaita and GStreamer versions;
- the interface you use, plus the output of `gaffer --list-ports`. **Remove the serial number**
  if you don't want to share it;
- what you expected, what happened, and how to reproduce it.

Please don't attach your whole `show.json` unless it's needed. If you do, remove anything private
from it, such as light or scene names.

## Development

```sh
cargo build -p gaffer        # debug build
cargo run -p gaffer          # run without installing
cargo test --workspace       # all tests (needs GStreamer base/good plugins)
cargo fmt --all
cargo clippy --workspace --all-targets
```

You don't need DMX hardware. Without an interface, Gaffer runs as a simulator and *Try sample
lights* gives you something to work with. The output crate's tests use a pseudo-terminal in place
of a real serial port.

Where things go:

- `core/`: engine logic (looks, scenes, effects, schedules, sound cues). Keep it free of I/O and
  GTK, and add unit tests for new behaviour there.
- `output/`: talking to USB-DMX interfaces. If you add support for an interface, say in the pull
  request which hardware you tested on.
- `app/`: the GTK4/libadwaita user interface, the `gaffer ctl` FIFO and the Touch Bar scenes.

## Pull requests

- One topic per pull request, with a short description of what changed and why.
- `cargo test --workspace` and `cargo clippy` should pass.
- Use plain, friendly words in UI text and messages, as in the existing app.
- `gaffer ctl` only accepts a fixed list of action names and never executes anything it reads.
  Please keep it that way.
- Any new user-visible feature should be described in `README.md`.

By contributing, you agree that your contribution is released under the project's license
(see `README.md`; it is still being chosen).
