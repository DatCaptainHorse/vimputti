### vimputti

An isolated input system (emulator? simulator?) for Docker/Podman/etc. containers running Linux.

Written in Rust cuz I'm lazy and don't want to deal with C/C++
(I hate dealing with C/C++ build systems and dependencies).

Name comes from the Finnish word/saying "himputti" which means "darn" or "dang it".
Which was my reaction to the whole situation.. someone smarter than me, go and make things better please!

#### Why?

Because I didn't want to pass `/dev/uinput` and other host mess into my container just to have virtual controllers.

Also I wanted more isolation between host and container.

#### How it works

vimputti consists of couple parts, the LD_PRELOAD shim library, the manager daemon and the
library API for applications to use.

##### LD_PRELOAD shim

The shim intercepts various API calls and redirects them to the manager.
It is buildable for both 32-bit and 64-bit needs (i.e. Steam requires 32-bit for some reason still).

##### Manager daemon

Manager handles socket messaging (by default via `/tmp/vimputti-0`) and manages virtual input devices
in the `/tmp/vimputti/` directory.

##### Library API

The library API is used by applications to super simply create various controller devices
and send input events to them. Currently Rust only, feel free to create a new issue for more bindings.

#### Building

##### Shim

64-bit: `cargo build --release --package vimputti-shim`
32-bit: `cargo build --release --package vimputti-shim --target i686-unknown-linux-gnu`

##### Manager daemon

`cargo build --release --package --package vimputti-manager`

#### TODOs (in no particular order)

- [ ] Rumble feedback support
- [ ] More bindings (C/C++, Go..)

#### Credits for wonderful people

- [Games-on-Whales](https://github.com/games-on-whales) - for their work on and insights on Linux input systems.
- [@ABeltramo](https://github.com/ABeltramo) - seriously man, thank you for being great and listening to my rants :P
- [@flumf](https://github.com/flumf) - for listening to me complaining about computers :3

#### License

MIT License, see `LICENSE` file for details.

It's open-source for a reason, everyone should be able to benefit from my weeks of pain.

#### Note

LLM(s) were ~~abused~~ used to generate some of the code, obviously I proofread it and fixed some junk and mistakes.

They're tools, and with how bad code they write, they won't replace caffeine-addicted programmers too soon.

#### Duck

Eater of bugs, judger of spaghetti code.
```
  __
<(o )___
 ( /_> /
  '---'
```
