# Third-party code in `mql5/`

Everything below is vendored unmodified. `scripts/install-ea.ps1` reads the SHA-256 table in this
file and refuses to install a DLL whose hash does not match, so update the table whenever a DLL changes.

## mql-zmq (ZeroMQ binding for MQL4/5)

- Project: https://github.com/dingmaotu/mql-zmq
- Licence: Apache-2.0 (copy in `Include/Zmq/LICENSE` and `Libraries/LICENSE.mql-zmq`)
- Source commit: `802c86c9e3647c917151ff724ffd15baa9bc152c` (2023-12-19, master at time of vendoring)
- Copied files:
  - `Include/Zmq/*.mqh` from `Include/Zmq/`
  - `Libraries/libzmq.dll`, `Libraries/libsodium.dll` from `Library/MT5/` (x64 builds)

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

The DLLs are committed to the repository on purpose (project decision). They are the prebuilt x64
binaries shipped by mql-zmq. They depend on the Visual C++ 2015+ runtime (`VCRUNTIME140.dll`,
`MSVCP140.dll`).

| File | Version | SHA-256 |
|---|---|---|
| libzmq.dll | libzmq 4.2.1 (file version 4.2.1.0) | 1ddbb1c390db6afc0e9dab99735ffa11444dba51e2d553a041c7c00692494170 |
| libsodium.dll | libsodium 1.0.11 (per mql-zmq README) | 648c35aba01dabab91cf4c85cdf008dc65b79c96682099534fa8662101526195 |

Verify manually:

```powershell
Get-FileHash mql5\Libraries\*.dll -Algorithm SHA256
```

Note: libzmq 4.2.x dates from 2016-2017. The bridge only binds to `127.0.0.1` and exposes read
commands, so the exposure is small, but plan to rebuild/upgrade the DLLs from a current libzmq
release before any non-localhost use.
