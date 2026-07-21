# Deterministic recording of the living-service hero GIF for the README.
#
# Requires charmbracelet/vhs (https://github.com/charmbracelet/vhs). Record with:
#
#     cargo build -p blaze-jit --release --example living_service   # pre-build once
#     vhs docs/living_service.tape                                  # -> docs/living_service.gif
#
# Pre-building keeps the capture to the run itself, not the compile. The tape
# then runs the six-beat `--script` story over real HTTP under load and waits
# for the final asserted line before finishing, so the GIF length adapts to the
# machine rather than being a fixed guess.

Output docs/living_service.gif

Require cargo

Set Shell "bash"
Set FontSize 13
Set Width 1200
Set Height 800
Set Padding 20
Set Theme "Dracula"
Set TypingSpeed 45ms
Set PlaybackSpeed 1.0

# Give it a moment of stillness, then type the command.
Sleep 800ms
Type "cargo run -p blaze-jit --release --example living_service -- --script"
Enter

# The dashboard animates through all six beats; then the summary prints. Wait
# for the last asserted line (up to 90s to be safe on a cold/slow box), then
# linger so the final frame reads.
Wait+Screen@90s /all six guarantees asserted/
Sleep 3s
