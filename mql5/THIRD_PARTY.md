# Third-party code in `mql5/`

The mql-zmq and mql4-lib sources below are vendored with one small local patch (described under
mql-zmq). `libzmq.dll` is built from the official libzmq source by `scripts/build-libzmq.ps1`.
`scripts/install-ea.ps1` reads the SHA-256 table in this file and refuses to install a DLL whose hash
does not match, so update the table whenever a DLL changes.

## mql-zmq (ZeroMQ binding for MQL4/5)

- Project: https://github.com/dingmaotu/mql-zmq
- Licence: Apache-2.0 (copy in `Include/Zmq/LICENSE` and `Libraries/LICENSE.mql-zmq`)
- Source commit: `802c86c9e3647c917151ff724ffd15baa9bc152c` (2023-12-19, master at time of vendoring)
- Copied files:
  - `Include/Zmq/*.mqh` from `Include/Zmq/`
  - The DLLs are not taken from mql-zmq any more: see "Binary DLLs" below.

### Local patch to the vendored mql-zmq headers (the only modification)

MetaEditor build 5.0.0.6230 rejects mixing `char[]` and `uchar[]` arrays (error 246 "parameter
convertion type ... is not allowed"), which the 2023 sources do in 26 places. We changed the
type `char` to `uchar` in code (never in comments or logic) in:
`Socket.mqh` (9 lines), `SocketOptions.mqh` (2), `Z85.mqh` (7), `ZmqMsg.mqh` (1), `Zmq.mqh` (1; this
file is UTF-16 with BOM, keep it that way). Both types are one byte wide, so the DLL calls are
unchanged. Re-apply this patch if the headers are ever re-vendored.

## mql4-lib (subset needed by mql-zmq)

- Project: https://github.com/dingmaotu/mql4-lib (a verbatim copy ships inside the mql-zmq repo)
- Licence: Apache-2.0 (copy in `Include/Mql/LICENSE`)
- Source: the `Include/Mql/Lang/` files of the same mql-zmq commit above:
  `Error.mqh`, `GlobalVariable.mqh`, `Mql.mqh`, `Native.mqh`

## Binary DLLs

The one DLL is committed to the repository on purpose (project decision). Since 2026-10-30 it is built
from source here, not copied from mql-zmq, to close security finding BR-2 (the old libzmq 4.2.1 and
libsodium 1.0.11 had CVE-2019-6250 and CVE-2020-15166). `libsodium.dll` is gone: no mql-zmq header
imports from it (the `#import` blocks name only `libzmq.dll`), and libzmq is built without CURVE.

| File | Version | SHA-256 |
|---|---|---|
| libzmq.dll | libzmq 4.3.5 (built from source, x64, static C runtime, no CURVE) | b49fbec77ab3856f3bddec5835d26f556ba60613da70df7d7d4f817cf303695a |

Verify manually:

```powershell
Get-FileHash mql5\Libraries\libzmq.dll -Algorithm SHA256
```

### How libzmq.dll is built

Run `powershell -ExecutionPolicy Bypass -File scripts\build-libzmq.ps1` (add `-RunTests` to also run
libzmq's own test suite). The script downloads, verifies, builds, checks and installs the DLL, and
prints the SHA-256 and whether it matches the table above. Building twice, even in different folders,
gives the same file for the same toolchain.

- Source: https://github.com/zeromq/libzmq/releases/download/v4.3.5/zeromq-4.3.5.tar.gz (official release, libzmq 4.3.5)
- Tarball SHA-256: `6653ef5910f17954861fe72332e68b03ca6e4d9c7160eb3a8de5a5a913bfab43`
  (pinned in the script; the release's own `SHA1SUMS` and `MD5SUMS` agree with the download)
- Licence: MPL-2.0, copy in `Libraries/LICENSE.libzmq` (from the tarball). We ship the unmodified
  libzmq build; its source is the tarball above.
- Build tools (the values used for the committed hash): Visual Studio Build Tools 2026 18.6, MSVC
  `cl` 19.51.36243.0 (toolset 14.51.36231), CMake 4.4.4, generator "Visual Studio 18 2026", x64, Release.
- Options: `WITH_LIBSODIUM=OFF`, `ENABLE_CURVE=OFF`, `ENABLE_DRAFTS=OFF`, `WITH_DOCS=OFF`,
  `BUILD_TESTS=OFF`, `BUILD_SHARED=ON`, `BUILD_STATIC=OFF`, `WITH_PERF_TOOL=OFF`, `ENABLE_CPACK=OFF`.
  Poller: the Windows default (wepoll, bundled in the source).
- C runtime: static (`/MT`), so the DLL needs no Visual C++ Redistributable. It imports only
  `KERNEL32`, `ADVAPI32`, `WS2_32` and `IPHLPAPI`; the script fails if any other DLL appears.
  Consequence: the DLL has its own C runtime, so only `zmq_errno()` (already what mql-zmq uses)
  reports its error codes. Do not read libzmq errors with the C `errno` of another module.
- Reproducibility: `/Brepro` and `/PDBALTPATH` on the linker and `/d1trimfile` on the compiler remove
  timestamps and the builder's paths from the binary.
- CURVE: not built. `zmq_has("curve")` returns 0, and `zmq_curve_keypair` / `zmq_curve_public` (declared
  in `Include/Zmq/Z85.mqh`) are still exported but fail with `ENOTSUP`. The EA does not use CURVE;
  bridge authentication is a message MAC (see the audit, BR-1). Z85 encode/decode still works.
- ABI: `zmq_msg_t` is 64 bytes in 4.3.5, the size `ZmqMsg` in `Include/Zmq/ZmqMsg.mqh` assumes. Every
  function the headers import (47) is exported by the DLL; the script checks this on every build.

To upgrade libzmq: change `$Version`, `$TarballUrl` and `$TarballSha256` in the script, run it, review
the output, then update this section and the table.
