# InkType

A speed-focused Remagic prototype: write on ruled paper and every pen or eraser
lift asks a fast vision model to reconcile the gesture with an editable typeset
document. Pending handwriting remains visible until the model conclusively
replaces it.

The model's entire response protocol is one line:

- `?` — wait for another stroke.
- `N,S,E:T` — consume `N` oldest pending gestures and replace character range
  `[S,E)` with `T`. An empty `T` deletes text.

Erasing never touches the model: an eraser lift deletes the typeset
characters whose glyph cells it swept and trims any pending ink underneath,
immediately. The deletion point stays visible as a thin caret; the next
handwriting near it anchors there, so writing into an erased spot replaces
the old text in place. Each request carries a per-line map of baseline `y` →
character offsets, and ink starting below the last typeset line is hinted as
a `\n`-prefixed insertion, so writing on lower ruled lines produces new lines.

Pen lifts are debounced for 180 ms, so fast handwriting is
sent as one recognition unit rather than one request per stroke. A large,
closed pen loop enters **circle command** mode and uses an 850 ms pause: circle
typeset text loosely, then write an action beside it. Transformations are
`sort`, `calculate`, `correct`, and `reflow`. A runtime selector routes the
exact canonical selection through rat: `run` is an alias for `py`; `py`, `r`,
`pi`, and registered names such as `my-py-here` behave like
`rat run <selector> <selection>` from `/home/root/inktype-repl`. Rat therefore
owns project scoping, named-runtime resolution, startup, environments, and
persistent state. The model only recognizes the selector and cannot change the
locally computed source range. The current tablet setup includes uv/Python (pandas and matplotlib), R 4.5
(jsonlite and ggplot2), Node, pi, tmux, and rat; the launcher reuses InkType's
Gemini key for pi's Google provider.

Circle a canonical table and write `df sales` (Python), `py df sales`, or
`r df sales`. Gemini returns validated columns/rows JSON; InkType creates the
named persistent pandas/R data frame through rat while leaving the source table
on the page. Circle later code and use that variable normally.

Matplotlib `plt.show()`, base-R graphics, and visible ggplot objects are captured
as PNG artifacts. InkType copies them from rat's cache into
`/home/root/inktype-assets/` and renders them inline as scalable, scrollable,
undoable image blocks. Their `×` removes the whole plot.

For reliable equation entry, tap the top `Math: OFF` button to switch to
`Math: ON`. Math mode waits 750 ms after the last stroke, then treats all
pending ink as one equation. Handwritten notation is stored with both display
LaTeX and an executable SymPy representation, then rendered as a cached typeset
equation. Tap the equation's `▶` button to evaluate it through the persistent
Python runtime; the exact result is inserted below as another LaTeX/SymPy math
block. Circle-and-write `calculate` remains available as an alternative.

Rat timing, runtime, and variable-count diagnostics are removed locally. Qwen
receives only the program's raw value or error, lightly presents it for paper,
and chooses the most natural insertion offset in the current document. The
operation is insertion-only, so source code remains intact. Long jobs survive
unrelated page edits and are attached only if their original source is still
present. Result blocks use a compact font and have a pen-tappable `×`
that removes the whole block. Runtime code runs with the app's tablet
permissions, so only run code/prompts you trust. `correct` handles either prose
or code and is instructed to preserve complete fenced-code backticks.

Touch navigation follows the tablet conventions: one-finger swipe moves 75%
of a page, two-finger drag scrolls continuously, two-finger tap undoes, and
three-finger tap redoes. Five fingers still exits.

New pen contact logically cancels any older response; stale calls may finish in the
background but can never edit the document. After `?`, InkType retries after
260 ms with a forced-decision hint. Two consecutive waits leave the ink in
place until the next gesture.

## Build and install

```sh
cd riddle/inktype
./build-takeover.sh
./make-bundle.sh
remagic install ./dist/inktype
remagic config inktype
```

Launch **InkType** from AppLoad or Remagic Home. Five fingers or the power
button exits. The document persists at `/home/root/inktype-document.txt`.

`INKTYPE_OPENAI_*` configures any OpenAI-compatible vision endpoint. The
default recognition model is `gemini-3.1-flash-lite`. Existing
`RIDDLE_OPENAI_*` variables are accepted as fallbacks; when InkType has no
`oracle.env`, its launch script also sources the installed Diary's config.
Requests use temperature zero, `reasoning_effort=none`, and a 128-token ceiling
(the protocol itself normally occupies only a handful of tokens).

Latency stages (`prepare_ms`, `model_ms`, and `paint_ms`) are written to the
journal for each gesture. The top-right page status shows `AI ready`,
`recognizing`, `waiting`, `recognized`, or `AI error`.

The last 20 calls are retained under `/home/root/inktype-debug/` as paired
`slot-NN.png`, `slot-NN.request.txt`, and `slot-NN.response.json` files. Inspect
them without stopping the app:

```sh
ssh rm 'journalctl -u inktype-takeover -n 100 -o cat'
scp -O -r rm:/home/root/inktype-debug ./inktype-debug
```
