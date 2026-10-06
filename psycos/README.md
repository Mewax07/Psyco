# PsycOS

**PsycOS** is a complete operating system written entirely in **Psyco**, a programming language designed specifically for this project, and compiled with **psycoc**, a custom compiler that produces its own native binary format.

The goal is simple: bring together the best qualities of the three major systems, without their compromises.

- **As complete and accessible as Windows**: all essential applications are built in, and everything is usable from the very first boot.
- **As lightweight and permissive as Linux**: minimal footprint, a fully open system, and total control for the user.
- **As elegant and secure as macOS**: a polished, animated and consistent interface, with security designed in from the ground up.

## Why a custom language and compiler?

To build a system that is truly consistent from the kernel up to the interface, the entire toolchain had to be under control. That is why PsycOS rests on three independent building blocks:

1. **Psyco** — the programming language, designed for performance and safety.
2. **psycoc** — the compiler, which turns Psyco code into optimized native binaries.
3. **PsycOS** — the operating system itself, running those binaries.

Nothing depends on an external tool: the language, the compiler and the binary format are all entirely dedicated to the system, enabling end-to-end optimization that would be impossible with a standard toolchain.
