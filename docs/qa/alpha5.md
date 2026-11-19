# Desktop transport and server switching: alpha.5

## Cause and change

TigerVNC selected raw rectangles in the measured 1280×800 desktop session,
producing 4,096,232 bytes before the first complete frame. Slow delivery could
also hit a fixed 45-second deadline and restart a download that was progressing.

The client now negotiates `zlib-chunks-v1` with the workspace manager. Each
independent chunk restores at most 65,536 original RFB bytes. The decoder checks
lengths, zlib completion and trailing bytes. Incompressible chunks use raw bytes.
There is no reduction in pixel quality. Peers without the negotiated transport
continue to exchange the existing raw RFB stream. The first-frame timeout now
measures inactivity rather than elapsed download time.

Switching connections parks only the recent chat and viewer placement, keyed by
server URL. At most two inactive servers retain up to 100 transcript rows and
16 MiB of estimated text/media memory each. No live Gateway connection or VNC
texture is retained in that cache. Empty chats and switches made before workspace
hydration preserve pending placement. A different selected chat clears the previous
preview. Server snapshots and bindings remain authoritative.

The viewer uses an aspect-aware skeleton during initial connection and keeps a
dimmed image when reopening a minimized viewer. Input requires a fresh frame and
a valid server-granted control lease. The old duplicate empty placeholder was
removed after a sizing regression reproduced a 1,122-pixel loading window.

## Checks performed

- 74 client tests passed; the unrelated live cron lifecycle test remains ignored.
- 22 manager tests passed. New coverage includes compression round trips, malformed
  packets, chunk boundaries, server isolation, bounded cache, empty/rapid switches,
  retained minimized image, loading geometry and narrow layout fit.
- Independent code review found two placement restoration gaps; both were fixed
  and covered by focused tests before packaging.
- Live authenticated video check: 147,512 transport payload bytes decoded to
  4,096,232 original bytes (96.4% reduction). First complete frame arrived 0.801 s
  after metadata; total TLS/authentication/frame time was 1.988 s in this sample.
  This is a single measured sample, not a latency guarantee.
- A client without transport negotiation received raw metadata and raw pixels
  from the updated manager.
- Native Bot Screen checks covered the wide viewer and a 390×780 compact preview,
  initial skeleton, live frame, minimize/reopen, and return to the original server
  with its recent transcript retained. This is a native egui app; Playwright is
  not applicable. Narrow 320/390/900 geometry also has automated coverage.
- Both existing server managers were deployed with rollback copies. Desktop and
  OpenClaw PIDs were unchanged by deployment; desktop memory/swap limits remained
  unlimited. Builds used a hard 2,000,000,000-byte cap, zero swap and one Cargo job.
- APK version code 10 uses the same signing certificate as the previous release.

## Limits

The phone was not connected to ADB, so this build was not installed or exercised
on physical Android during this pass. The compact native preview does not verify
Android IME, touch or system bars. The test Linux device was not approved on the
second Gateway: its approval requirement was respected, and only return-to-first-
server behavior was exercised there. Full authenticated switching between both
Gateways on the already-approved Android device remains a device follow-up.
Network loss and server outages can still delay a connection.
