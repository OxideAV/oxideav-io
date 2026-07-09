# oxideav-io benchmarks

Criterion benches for the probe hot paths live in `benches/probe.rs`.
Every fixture is synthesized on the fly (facade save path or a
hand-rolled canonical WAV header); nothing is committed. Run with:

```sh
cargo bench -p oxideav-io --bench probe
```

## Baseline (2026-07-09, Apple Silicon, `-j 4`, default profile)

| bench                  | time      | what it isolates |
|------------------------|-----------|------------------|
| `ping_png_4x4`         | ~6.1 µs   | fast-path hit on a ~100 B image: magic peek + probe window over a tiny file |
| `ping_wav_4mib`        | ~1.24 ms  | fast-path hit on a large file — cost is the 257 KiB read-budget window, **not** the 4 MiB file |
| `ping_noise_256k_miss` | ~7.8 ms   | fast-path miss: every registered container probe scans the full 256 KiB window with no early exit |
| `probe_png_4x4`        | ~6.5 µs   | full probe (adds demuxer open + stream-table parse) |
| `probe_wav_4mib`       | ~1.24 ms  | full probe on the large file — the WAV header parse is negligible next to the probe window |

## Reading the numbers

* **Ping cost does not scale with file size** — the 4 MiB WAV pings in
  the same ~1.24 ms a 300 KiB one would, because the enforced
  `PING_FORMAT_MAX_READ_BYTES` budget caps the window. That is the
  contract `tests/ping_contract.rs` pins.
* **A miss is ~6× a large-file hit**: rejection means *every* probe in
  the registry scans the window to its end (sync-scan style probes have
  no early exit on garbage). If "identify or reject quickly" ever
  becomes a hot requirement, the leverage is in the registry's probe
  loop, not in this facade.
* **`probe` ≈ `ping` + header parse**: for header-at-front containers
  the demuxer open adds single-digit µs on small files and vanishes in
  the noise on large ones. The two-tier split pays off mainly on
  corrupt-body inputs (ping succeeds where probe must fail) and on
  containers whose stream table sits far into the file.
