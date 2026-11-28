# D4C patch to epaint 0.37.0

Source is the epaint 0.37.0 crate from crates.io. We clear the first-row
indentation after a normal line break. Otherwise a long word on a later row
can reapply the indentation and shift the rendered text outside the chat
column. The `compact_transcript_keeps_text_inside_viewport` test covers this
case through the D4C chat renderer.
