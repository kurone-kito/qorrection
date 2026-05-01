# Trigger Pipeline Policy

This document pins the v0.1 behavior for trigger detection edges
that are easy to change accidentally while wiring the real PTY
pump.

## Bracketed Paste

`qorrection` is observer-only for DEC private mode 2004:

- it does not emit `ESC[?2004h` or `ESC[?2004l`;
- it does not strip `ESC[200~` / `ESC[201~` markers;
- it does not synthesize markers around pasted bytes.

When the wrapped child enables bracketed-paste mode, the terminal
sends the begin/end markers in the user's input stream. The input
pump forwards those marker bytes to the child unchanged, while
`PasteTracker` uses them to temporarily bypass the trigger parser.
This prevents pasted source text containing `:q`, `:wq`, or `:q!`
from firing an animation.

When the child does not enable bracketed-paste mode, pasted text
is indistinguishable from typed text. In that case a pasted `:q`
line is treated exactly like a typed `:q` line.

## Pump Contract

`trigger::input::InputPump` is the canonical pure state machine for
this contract. The real host-input pump feeds user bytes through
`InputPump::feed_input_byte`, while the output side feeds child bytes
through `InputPump::feed_child_output_byte` so alternate-screen state
can disarm input parsing.

`trigger::input::InputDetector` is the Phase 3 detect-only host-input
adapter. It forwards bytes unchanged and emits `tracing::info!` only
when `InputPump` reports a trigger outcome.

`trigger::output::OutputArbiter` is the canonical child-output
adapter. It forwards child output unchanged and feeds the bytes that
were accepted by the host writer into the shared `InputPump`.

The input pump must snapshot tracker state before each byte,
feed the byte into `PasteTracker`, and bypass `Parser` when either
the pre-byte or post-byte paste state is active. It must also call
`Parser::reset` on every transition into or out of paste mode.

That pre/post rule keeps the marker bytes themselves away from
the parser, including the trailing `~` of `ESC[201~`, which is the
byte that flips the tracker back to non-paste mode.
