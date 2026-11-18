# Android Activity lifecycle patch

Source: winit 0.30.13, rust-windowing contributors.
https://github.com/rust-windowing/winit/tree/v0.30.13
Copied from the installed crates.io source on 2026-11-18.
License: Apache-2.0; upstream LICENSE and source copyright notices are retained.

Changes are confined to Android:
- MainEvent::Destroy exits the event loop so android_main returns, unblocking
  NativeActivity.onDestroy on Java's main thread.
- Whenever the loop exits (including a window close before Destroy), release the
  creation guard before LoopExiting cleanup. This permits a replacement Activity
  with its own AndroidApp. Other platforms retain their existing guard.

The foreground notification service may retain the Android process. Never call
process::exit or kill the service as a workaround for Activity teardown.

Physical verification must recreate the Activity/task without force-stopping the
process; a force-stop alone cannot reproduce this lifecycle defect.
