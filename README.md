# muir-sim

The simulator of project muir's two machines: QUUX, the CADR evolved, and
the MIT CADR Lisp Machine itself, down to the chips.

The CADR is the second-generation MIT Lisp Machine, designed around 1978 by
Tom Knight, David Moon, Jack Holloway and Guy Steele. muir-sim models it far
enough down that the software written *for the hardware* runs --- the MIT
diagnostics, the console program CC, and two machines lashed together with
one debugging the other. That lashup is the acceptance test, and it passes.
QUUX is the same processor, buses and boards, changed where a change pays
for itself, and runs muir-sys's system for it; [QUUX](docs/quux.md) says
where it differs. There are three engines: `micro`, the fastest, with no
timing model; `rtl`, on the machine's own clock, for ordinary use; and
`chip`, which runs MIT's own drawings part by part, for the CADR. Rust, no
crate dependencies.

The site is **[muir.metebalci.com/simulator/](https://muir.metebalci.com/simulator/)**,
and [the manual](docs/manual.md) is the rest.

## Quick start

Install `rustup`, then open a new shell so `cargo` is on the path:

    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

Build muir-sim and fetch QUUX's system, the current release of
[muir-sys](https://github.com/metebalci/muir-sys), System 2001, which runs
on QUUX revision 13, the 40-bit word. The compiler version is pinned in
`rust-toolchain.toml`, and the release lands in `vendor/`, every file
checked against its SHA-256 sum.

    git clone https://github.com/metebalci/muir-sim
    cd muir-sim
    cargo build --release
    tools/fetch-system-for-quux.sh

The script puts QUUX's disk in `vendor/run/` and makes the machine's host
folder beside it: copies of the release's `sys/` and `site/`, and an empty
home to log in to. QUUX reads its files from that folder through a device
of its own, so it needs no file server. Start the machine:

    target/release/quux --micro --disk-pack vendor/run/release-2001-disk.vhd \
        --file-root vendor/run/release-2001-root

It says where its terminal is and boots. Point any VNC viewer at
`vnc://127.0.0.1:5900`, with no password: the display, keyboard and mouse
are the machine's only way in or out. It boots to a Lisp Listener, System
2001; `(login "LISPM" "HOST" t)` logs in. `--micro` is the faster engine;
without it `quux` runs `rtl`, at the machine's own speed.

## The CADR

The CADR is the reference: MIT's machine as built, running MIT's last
release, [System 100](https://tumbleweed.nu/system-100-0-release/). Fetch
it:

    tools/fetch-system-100-for-cadr.sh

A Lisp Machine took its files and the date from a host on the Chaosnet, and
a CADR had no such server in it, so muir-sim has none either.
[ozd](https://github.com/metebalci/ozd) is that host. Build it beside muir-sim
and start it in a shell of its own, serving System 100's sources:

    git clone https://github.com/metebalci/ozd ../ozd
    (cd ../ozd && cargo build --release)
    mkdir -p vendor/run/oz/lispm
    ../ozd/target/release/ozd --address 3060 --name MIT-OZ,OZ,system=UNIX \
        --root $PWD/vendor/run/oz --root tree=$PWD/vendor/system-100-0/sys,ro \
        --file-dates mit --timezone 5

`--file-dates mit --timezone 5` has ozd give a file's dates as System 100
writes them, in its site's zone, 5 hours west of Greenwich
(`sys/site/site.lisp:99`, `(:TIMEZONE 5)`). ozd's default, plain UTC, is
for System 1002 and later, and a band of System 100 read through it has
every file date off by that zone.

Then start the machine. System 100's host table puts the machine at 3050 and
its host at 3060; ozd has UDP port 42042, so the machine takes 42043.

    target/release/cadr --disk-pack vendor/run/disk-sys-100-0.img \
        --chaos-address 3050 --chaos-udp 42043 --chaos-udp-peer 3060@127.0.0.1:42042

It says where its terminal is and boots, on the same VNC terminal. In under
a minute it is at a Lisp Listener with the date set. Type `(si:setup-cpt)`
there for the mouse, which [the terminal](docs/manual.md#the-terminal)
explains, along with what to type when a boot with no host stops to ask for
the date.

muir-sys also evolves a system for the CADR, on the CADR's own line of
releases: `tools/fetch-system-for-cadr.sh` fetches its current one, and its
release notes give the ozd and `cadr` commands.

## Documentation

| | |
|---|---|
| [QUUX](docs/quux.md) | Where QUUX differs from the CADR, and what each difference needed of the boot PROM, the microcode and the system |
| [The manual](docs/manual.md) | Running muir-sim: what a run says, every flag, flags in a file, the prompt, the terminal, what a run can write, the Chaosnet and the two-machine lashup |
| [Making a pack](docs/diskpack.md) | `diskpack`, the third binary: a pack of one's own, its partitions, and bands loaded and dumped |
| [How the engines work](docs/engines.md) | `micro`, `rtl` and `chip`: what each computes, how fast each runs, and what each models board by board |
| [The machine it models](docs/machine.md) | The processor, the bus interface, the two buses and every board on them |
| [Where the netlists come from](docs/netlists.md) | MIT's drawings and wire lists, how a board becomes a netlist, and what each is checked against |
| [The Chaosnet board](docs/chaosnet.md) | The interface on the I/O board, its cable, and what muir-sim has of it |
| [The TV board](docs/tv.md) | The SIMPLE TV, the LISPM TV and the color TV |
| [The keyboard boot sequence](docs/keyboard-boot.md) | The chord that boots the machine, from the keyboard's firmware to the processor |
| [Sources and attribution](docs/sources.md) | What this is built on, related projects, how it was written, and the license |

## Layout

    mit/       MIT's own files, unmodified: the drawings and wire lists of
               every board modeled here, and a snapshot of System 100's own
               sys tree, which the boot PROM comes from. mit/README.md
    data/      what is made from mit/ by a script in tools/, each
               cross-checked or labeled: the eight netlists, the disk
               controller's microcode, the cable tables. data/README.md
    src/       the simulator, its two executables cadr and quux, and
               diskpack
    tests/     the checks
    examples/  development tools; none is part of the simulator.
               examples/README.md
    tools/     fetching the releases, and one script per board to
               re-extract a netlist. tools/README.md
    docs/      the manual, and findings about the machine read from mit/,
               each claim cited to its file and held by a test where one
               can. docs/README.md
    vendor/    fetched material, never committed

## License

Copyright (C) 2026 Mete Balci. Free software under the **GNU Affero General
Public License**, version 3 or, at your option, any later version; see
`LICENSE`. `mit/` is MIT's work and `tools/soap4/` came from `ams/cadr4`:
[the license](docs/sources.md#license) says what is whose.

## How it was written

muir-sim is implemented entirely by [Claude Code](https://claude.com/claude-code),
on Anthropic's Opus and Fable models.
