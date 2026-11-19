# Alpha 6 verification

## Root causes

Doom held an old Codex 0.164 process after 0.169 was installed. Its cached account
catalog rejected GPT 6.2 models. Restarting the idle Gateway loaded 0.169 and
refreshed the account catalog; both 6.2 Sol and 6.2 Astra then completed real chats.
A custom service override also imposed MemoryHigh=2 GB / MemoryMax=3 GB and caused
heavy memory pressure. That override was backed up and removed on Doom. D4C does
not impose these Gateway runtime limits. Compilation still has a hard 2 GB limit,
zero build swap and one Cargo job.

The client discarded displayable custom error messages, hiding failures from the
conversation. It now renders those messages, handles live failures and recovers
missed failures from a session snapshot, using run IDs to deduplicate notices.

## Checked

- 78 client tests passed (one unrelated live cron test remains ignored).
- 23 server tests passed, including recovery notification isolation/idempotency.
- 10 Node recovery tests passed: active work guards, context run IDs, cooldown,
  disposal, hourly limits, non-timeout exclusions and real process termination.
- 6 Python catalog tests passed: provider isolation, hidden models, fresh catalogs
  and refusal to guess default replacements under restricted policy.
- Actual Doom replies: D4C_DOOM_OK, D4C_62_OK, D4C_ASTRA_OK and D4C_RECOVERY_READY.
- Managed connection refresh drill: three idle Gateway-owned Codex child processes
  exited; the Gateway PID and all desktop processes remained running. A subsequent
  real Astra chat returned D4C_RECONNECTED_OK, verifying re-establishment.
- Native Linux UI inspected at 1440x900 and 390x780, including error and success rows.
- Both workspace managers and desktop plugins updated; existing desktops preserved.
- Tusk and Doom authenticated Gateway probes passed after activation. Tusk's full
  cold startup took approximately 83 seconds; an early probe reset before readiness.
- Linux client/server and Android arm64 built under the enforced build guard.

## Limits

No automatic replay of a failed prompt or tool action. Connection refresh is bounded
and waits for tracked turns to finish. Unsupported models, missing managed children,
permission failures and unresponsive processes may still need operator action.
The recovery tests include synthetic timeout events; the live drill explicitly
refreshed idle managed children without deliberately timing out a production turn.
Physical Android installation/testing was unavailable because the phone disconnected.
The 390x780 check is a native Linux compact viewport, not an Android device test.
