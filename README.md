### vimputti

An isolated input system (emulator? simulator?) for Docker/Podman/etc. containers running Linux.

Written in Rust cuz I'm lazy and don't want to deal with C/C++

Name comes from the Finnish word/saying "himputti" which means "darn" or "dang it". Which was my reaction to the whole situation.. someone smarter than me, go and make things better please!

#### Why?

Because I didn't want to pass `/dev/uinput` and other host mess into my container just to have virtual controllers.

Also I wanted more isolation between host and container.

#### How it works

vimputti consists of two parts, the LD_PRELOAD shim library and the manager application.

The shim intercepts various input API calls and redirects them to the manager with socket connection.

Manager handles socket messaging (by default via `/run/user/N/vimputti-0`) and manages virtual input devices.

#### Building

`cargo build --release --lib` for the shim library, `cargo build --release --bin vimputti-manager` for the manager.
