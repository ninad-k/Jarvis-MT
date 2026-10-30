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

## Update 2026-10-30: libzmq 4.3.5 built from source, libsodium removed

Security finding BR-2 (see [the audit](../security/audit-2026-10-30.md)) showed that the prebuilt libzmq 4.2.1 carries CVE-2019-6250 (code execution through the ZMTP v2 decoder, fixed in 4.3.1) and CVE-2020-15166 (connection exhaustion, fixed in 4.3.3), and that libsodium 1.0.11 is old. The decision above still holds (the DLL is committed, with a checksum), but where it comes from has changed:

- `libzmq.dll` is now libzmq **4.3.5**, built from the official release tarball by [scripts/build-libzmq.ps1](../../scripts/build-libzmq.ps1). The script pins the tarball URL and its SHA-256, refuses to build from anything else, builds with the MSVC toolchain, and checks the result: x64, imports only Windows system DLLs, and exports every function the mql-zmq headers import.
- The build is configured without libsodium and without CURVE. The EA never used CURVE, and the bridge is protected by a message MAC instead. **`libsodium.dll` is deleted**, and the install script no longer needs it. No mql-zmq header imported from it; only the old libzmq loaded it as a dependency.
- The C runtime is linked statically (`/MT`), so the DLL needs only Windows system DLLs. This removes the Visual C++ runtime requirement listed under "Consequences" above.
- The build is repeatable: the same toolchain gives the same SHA-256 from any folder. [mql5/THIRD_PARTY.md](../../mql5/THIRD_PARTY.md) records the tarball hash, the options, the compiler version and the DLL hash, and the script reports whether a rebuild matches the table. libzmq's licence (MPL-2.0) ships next to the DLL as `Libraries/LICENSE.libzmq`.
- The mql-zmq headers needed no change for this. The earlier `char` to `uchar` patch is still the only local modification.
- The "build libzmq from source" alternative below is no longer rejected for the maintainer: it is how the DLL is produced. Users still install the committed DLL and need no compiler; only someone who updates libzmq needs CMake and the Visual Studio C++ build tools.

Checks run for this change: libzmq's own test suite (72 of 74 pass; the other two bind port 5555, which the running bridge held), a C reply server using the built DLL against the pure-Rust `zeromq` client with single-frame, two-frame (MAC plus payload), 64 KB and three-frame messages, and a MetaEditor compile of the EA with 0 errors and 0 warnings.
