# ADR-0004: Commit the ZeroMQ DLLs with SHA-256 checksums

- Status: Accepted
- Date: 2026-10-30

## Context

`JarvisBridge` needs the native libraries `libzmq.dll` and `libsodium.dll` at run time. They come from the mql-zmq project, which last released in 2019 and bundles libzmq 4.x. MT5 loads them from its data folder, and the EA cannot build them. Users on Windows need a setup that works without a compiler, and that does not run an unchecked binary.

## Decision

- The prebuilt x64 DLLs are committed under `mql5/Libraries/`: libzmq 4.2.1 and libsodium 1.0.11, as shipped by mql-zmq. The project approved committing them on 2026-10-30.
- [mql5/THIRD_PARTY.md](../../mql5/THIRD_PARTY.md) lists the source commit, the license, and the SHA-256 hash of each DLL.
- `scripts/install-ea.ps1` checks each DLL against that table before it copies the files, and again after the copy. It refuses to install a file whose hash does not match or that has no entry.
- The vendored mql-zmq headers carry one small local patch (`char` to `uchar`, needed by MetaEditor 5.0.0.6230). It is documented in THIRD_PARTY.md.

## Consequences

- Installation works offline and needs no C++ toolchain.
- The repository contains binaries, which reviewers cannot read. The checksum table makes changes visible, and updates must change the DLLs and the table together.
- The libzmq build dates from 2016 to 2017, so it has known age. The bridge binds to loopback and exposes only read commands, which limits the exposure. THIRD_PARTY.md says to upgrade before any use beyond localhost.
- The DLLs depend on the Visual C++ runtime, which the user must have installed.

## Alternatives considered

- **Download the DLLs during installation.** This needs network access, and it still needs a checksum from somewhere, so the trust decision moves rather than goes away.
- **Build libzmq from source.** It needs CMake and a C/C++ compiler on every user's machine, which is a high barrier for a single-user Windows setup.
- **Ask users to supply the DLLs.** This leaves the most common setup error to the user, with no checks.
- **Use the `zmq` Rust crate instead of MQL5.** The EA still needs the native library, so this does not remove the problem. It is kept as a fallback in the Sprint 1 risk list.
