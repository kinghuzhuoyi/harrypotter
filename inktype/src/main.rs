//! InkType: every pen/eraser lift asks a fast vision model to reconcile the
//! visible gesture with a canonical, reflowing text document.

#[path = "../../riddle/src/display.rs"]
mod display;
#[path = "../../riddle/src/fb.rs"]
mod fb;
mod oracle;
#[path = "../../riddle/src/pen.rs"]
mod pen;
#[path = "../../riddle/src/power.rs"]
mod power;
#[path = "../../riddle/src/qtfb.rs"]
mod qtfb;
mod repl;
#[path = "../../riddle/src/surface.rs"]
mod surface;
#[path = "../../riddle/src/touch.rs"]
mod touch;

use ab_glyph::{Font, FontRef, Glyph, PxScale, ScaleFont};
use oracle::{Decision, Request};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use surface::{Surface, BLACK, WHITE};

const FONT: &[u8] = include_bytes!("../fonts/DejaVuSans.ttf");
const W: usize = 1620;
const H: usize = 2160;
const LEFT: i32 = 92;
const RIGHT: i32 = 92;
const TOP: i32 = 150;
const FONT_PX: f32 = 62.0;
const OUTPUT_FONT_PX: f32 = 38.0;
const LINE_STEP: i32 = 94;
const OUTPUT_LINE_STEP: i32 = 58;
const RULE_OFFSET: i32 = 18;
// Text recognition launches on every pen lift. Uncertain strokes remain in
// `pending` until a later request can consume them conclusively.
const DEFAULT_DEBOUNCE: Duration = Duration::ZERO;
/// A circle is followed by a handwritten action word, so allow time to write it.
const COMMAND_DEBOUNCE: Duration = Duration::from_millis(850);
const WAIT_RETRY: Duration = Duration::from_millis(260);
const ERROR_RETRY: Duration = Duration::from_millis(420);
const MAX_FORCED_WAITS: u8 = 2;
const MATH_DEBOUNCE: Duration = Duration::from_millis(350);
static MATH_MODE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Pen,
    Eraser,
}

struct Gesture {
    tool: Tool,
    points: Vec<(i32, i32, i32)>,
}

#[derive(Clone, Copy)]
struct Pos {
    index: usize,
    x: i32,
    baseline: i32,
}

/// Screen-space box of one typeset character.
#[derive(Clone, Copy)]
struct Cell {
    index: usize,
    x0: i32,
    x1: i32,
    baseline: i32,
}

/// One visual line of the typeset document (wrap- or newline-delimited).
#[derive(Clone, Copy)]
struct Line {
    baseline: i32,
    start: usize,
    end: usize,
}

struct Layout {
    insertions: Vec<Pos>,
    cells: Vec<Cell>,
    lines: Vec<Line>,
    output_closes: Vec<OutputClose>,
    math_calculates: Vec<MathCalculate>,
    content_height: i32,
}

#[derive(Clone, Copy)]
struct OutputClose {
    x: i32,
    y: i32,
    size: i32,
    start: usize,
    end: usize,
}

#[derive(Clone, Copy)]
struct MathCalculate {
    x: i32,
    y: i32,
    size: i32,
    start: usize,
    end: usize,
}

#[derive(Clone, Copy)]
struct OutputRange {
    start: usize,
    end: usize,
    delete_start: usize,
    delete_end: usize,
}

struct ImageRange {
    start: usize,
    end: usize,
    delete_start: usize,
    delete_end: usize,
    path: String,
}

struct MathRange {
    start: usize,
    end: usize,
    delete_start: usize,
    delete_end: usize,
    payload: String,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("inktype: fatal: {e}");
        std::process::exit(1);
    }
}

fn run() -> std::io::Result<()> {
    let font = FontRef::try_from_slice(FONT).map_err(std::io::Error::other)?;
    let (disp, mut surf) = display::Display::open()?;
    let takeover = matches!(disp, display::Display::Quill);
    let mut pen = pen::PenDevice::open()?;
    let mut touch = if takeover {
        touch::TouchDevice::open().ok()
    } else {
        None
    };
    let mut power = if takeover {
        power::PowerButton::open().ok()
    } else {
        None
    };

    let quit = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&quit))?;
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&quit))?;

    let path = std::env::var("INKTYPE_DOCUMENT")
        .unwrap_or_else(|_| "/home/root/inktype-document.txt".into());
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut pending: Vec<Gesture> = Vec::new();
    let mut current: Option<Gesture> = None;
    let mut scroll_y = 0i32;
    let mut undo: Vec<String> = Vec::new();
    let mut redo: Vec<String> = Vec::new();
    let mut layout = render_all(&mut surf, &font, &text, &pending, scroll_y);
    paint_status(&mut surf, &font, "AI ready");
    disp.full_refresh(W, H);

    let oracle = match oracle::Oracle::from_env() {
        Ok(o) => Some(o),
        Err(e) => {
            eprintln!("inktype: oracle disabled: {e}");
            None
        }
    };
    let (tx, rx) = mpsc::channel::<oracle::Reply>();
    let runner = repl::Runner::from_env();
    let rat_runtimes = runner.runtime_names();
    eprintln!("inktype: rat selectors={rat_runtimes}");
    let (run_tx, run_rx) = mpsc::channel::<repl::Reply>();
    let (place_tx, place_rx) = mpsc::channel::<oracle::PlacementReply>();
    let mut placement_id = 0u64;
    let mut request_id = 0u64;
    let mut latest_request = 0u64;
    let mut run_id = 0u64;
    let mut input_revision = 0u64;
    let mut text_revision = 0u64;
    let mut dispatch_due: Option<(Instant, bool)> = None;
    let mut forced_waits = 0u8;
    let debounce = std::env::var("INKTYPE_DEBOUNCE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_DEBOUNCE);
    let mut dirty: Option<(i32, i32, i32, i32)> = None;
    let mut last_flush = Instant::now();
    // Insertion point left behind by the most recent local erase. New
    // handwriting anchors here so it replaces what was rubbed out.
    let mut caret: Option<usize> = None;
    let mut pen_in_proximity = false;
    let mut touch_blocked_until = Instant::now();

    eprintln!(
        "inktype: open; document={} chars={} display={}",
        path,
        text.chars().count(),
        if takeover { "quill" } else { "qtfb" }
    );

    while !quit.load(Ordering::Relaxed) {
        // Drain the marker first so palm rejection takes effect in the same
        // frame that the pen enters digitizer range, before touch is handled.
        let pen_samples = pen.drain();
        for sample in &pen_samples {
            pen_in_proximity = sample.proximity;
            if sample.proximity {
                // Keep palms suppressed briefly after hover ends; the touch
                // controller can report a delayed release frame.
                touch_blocked_until = Instant::now() + Duration::from_millis(180);
            }
        }
        let suppress_touch =
            pen_in_proximity || current.is_some() || Instant::now() < touch_blocked_until;
        let touch_events = if let Some(touch) = touch.as_mut() {
            if suppress_touch {
                touch.suppress();
                Vec::new()
            } else {
                touch.drain()
            }
        } else {
            Vec::new()
        };
        let mut quit_touch = false;
        let mut scroll_delta = 0i32;
        let mut smooth_scroll = false;
        let mut history_changed = false;
        for event in touch_events {
            match event {
                touch::Gesture::Quit => quit_touch = true,
                touch::Gesture::Scroll(delta) => {
                    scroll_delta += delta;
                    smooth_scroll = true;
                }
                touch::Gesture::Page(direction) => {
                    scroll_delta += direction * H as i32 * 3 / 4;
                }
                touch::Gesture::Undo => {
                    if let Some(previous) = undo.pop() {
                        redo.push(text.clone());
                        text = previous;
                        history_changed = true;
                    }
                }
                touch::Gesture::Redo => {
                    if let Some(next) = redo.pop() {
                        undo.push(text.clone());
                        text = next;
                        history_changed = true;
                    }
                }
            }
        }
        if quit_touch {
            break;
        }
        if history_changed {
            pending.clear();
            current = None;
            caret = None;
            dispatch_due = None;
            input_revision += 1;
            text_revision += 1;
            if let Err(e) = std::fs::write(&path, &text) {
                eprintln!("inktype: save: {e}");
            }
        }
        if scroll_delta != 0 || history_changed {
            if history_changed {
                // Measure the restored document before clamping its viewport.
                layout = render_all(&mut surf, &font, &text, &pending, scroll_y);
            }
            let max_scroll = (layout.content_height - H as i32 + 120).max(0);
            let old_scroll = scroll_y;
            scroll_y = (scroll_y + scroll_delta).clamp(0, max_scroll);
            let moved = scroll_y - old_scroll;
            if moved != 0 {
                for gesture in &mut pending {
                    for point in &mut gesture.points {
                        point.1 -= moved;
                    }
                }
            }
            layout = render_all(&mut surf, &font, &text, &pending, scroll_y);
            paint_status(
                &mut surf,
                &font,
                if history_changed { "history" } else { "scroll" },
            );
            if smooth_scroll && !history_changed {
                disp.update(0, 0, W as i32, H as i32, true);
            } else {
                disp.full_refresh(W, H);
            }
            dirty = None;
        }
        if power.as_mut().is_some_and(|p| p.drain_pressed()) {
            break;
        }

        for sample in pen_samples {
            let down = sample.touching && sample.pressure > 40;
            if down {
                let tool = if sample.tool == pen::Tool::Eraser {
                    Tool::Eraser
                } else {
                    Tool::Pen
                };
                let r = if tool == Tool::Eraser {
                    22
                } else {
                    2 + sample.pressure * 3 / pen::MAX_PRESSURE
                };
                if current.is_none() {
                    // Any response for an older visual snapshot is now stale,
                    // even before this new gesture has lifted.
                    input_revision += 1;
                    dispatch_due = None;
                }
                let gesture = current.get_or_insert_with(|| Gesture {
                    tool,
                    points: Vec::new(),
                });
                let (x, y) = (sample.x, sample.y);
                if let Some(&(px, py, pr)) = gesture.points.last() {
                    surf.brush_line(
                        px,
                        py,
                        x,
                        y,
                        r.min(pr + 1),
                        if tool == Tool::Pen { BLACK } else { WHITE },
                    );
                    dirty_add(&mut dirty, px, py, pr + 3);
                } else {
                    surf.stamp(x, y, r, if tool == Tool::Pen { BLACK } else { WHITE });
                }
                dirty_add(&mut dirty, x, y, r + 3);
                gesture.points.push((x, y, r));
            } else if let Some(g) = current.take() {
                if g.points.is_empty() {
                } else if in_reset_button(&g) {
                    // Debug escape hatch: tap the top-left button to wipe
                    // the document and all pending ink.
                    checkpoint(&mut undo, &mut redo, &text);
                    text.clear();
                    pending.clear();
                    MATH_MODE.store(false, Ordering::Relaxed);
                    scroll_y = 0;
                    caret = None;
                    current = None;
                    dispatch_due = None;
                    forced_waits = 0;
                    text_revision += 1;
                    input_revision += 1;
                    if let Err(e) = std::fs::write(&path, &text) {
                        eprintln!("inktype: save: {e}");
                    }
                    layout = render_all(&mut surf, &font, &text, &pending, scroll_y);
                    paint_status(&mut surf, &font, "cleared");
                    disp.full_refresh(W, H);
                    dirty = None;
                    eprintln!(
                        "inktype: reset tapped; document cleared text_revision={text_revision}"
                    );
                } else if in_mode_button(&g) {
                    let enabled = !MATH_MODE.load(Ordering::Relaxed);
                    MATH_MODE.store(enabled, Ordering::Relaxed);
                    pending.clear();
                    dispatch_due = None;
                    layout = render_all(&mut surf, &font, &text, &pending, scroll_y);
                    paint_status(&mut surf, &font, if enabled { "math mode" } else { "text mode" });
                    disp.full_refresh(W, H);
                    dirty = None;
                } else if let Some((start, end)) = math_calculate_hit(&layout, &g) {
                    let chars: Vec<char> = text.chars().collect();
                    let selected: String = chars[start..end].iter().collect();
                    if let Some(payload) = extract_math_payload(&selected) {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) {
                            if let Some(semantic) = value.get("semantic").and_then(serde_json::Value::as_str) {
                                let semantic_literal = serde_json::to_string(semantic).unwrap();
                                let execution = math_calculation_code(&semantic_literal);
                                run_id += 1;
                                runner.run(run_id, "py".into(), start, end, execution, selected, run_tx.clone());
                                paint_status(&mut surf, &font, "math: calculating");
                                disp.update(STATUS_X as i32, 0, STATUS_W as i32, STATUS_H as i32, true);
                            }
                        }
                    }
                } else if let Some((start, end)) = output_close_hit(&layout, &g) {
                    checkpoint(&mut undo, &mut redo, &text);
                    let mut chars: Vec<char> = text.chars().collect();
                    if start <= end && end <= chars.len() {
                        chars.drain(start..end);
                        text = chars.into_iter().collect();
                        text_revision += 1;
                        input_revision += 1;
                        pending.clear();
                        caret = None;
                        dispatch_due = None;
                        if let Err(e) = std::fs::write(&path, &text) {
                            eprintln!("inktype: save: {e}");
                        }
                        layout = render_all(&mut surf, &font, &text, &pending, scroll_y);
                        paint_status(&mut surf, &font, "output deleted");
                        disp.full_refresh(W, H);
                        dirty = None;
                    }
                } else if g.tool == Tool::Eraser {
                    // Erasing is deterministic: we know exactly which glyph
                    // cells and which pending ink the eraser covered, so
                    // resolve it locally instead of asking the model.
                    erase_pending_ink(&mut pending, &g);
                    let mut erased = None;
                    if let Some((start, end)) = erased_range(&layout, &g) {
                        checkpoint(&mut undo, &mut redo, &text);
                        let mut chars: Vec<char> = text.chars().collect();
                        chars.drain(start..end);
                        text = chars.into_iter().collect();
                        text_revision += 1;
                        caret = Some(start);
                        erased = Some((start, end));
                        if let Err(e) = std::fs::write(&path, &text) {
                            eprintln!("inktype: save: {e}");
                        }
                    }
                    layout = render_all(&mut surf, &font, &text, &pending, scroll_y);
                    if let Some(c) = caret {
                        draw_caret(&mut surf, &layout, c);
                    }
                    paint_status(&mut surf, &font, "erased");
                    disp.update(
                        0,
                        TOP - FONT_PX as i32,
                        W as i32,
                        H as i32 - TOP + FONT_PX as i32,
                        true,
                    );
                    dirty = None;
                    forced_waits = 0;
                    dispatch_due = if pending.is_empty() {
                        None
                    } else {
                        Some((Instant::now() + debounce, false))
                    };
                    eprintln!(
                        "inktype: eraser applied locally erased={erased:?} caret={caret:?} pending={} text_revision={text_revision}",
                        pending.len()
                    );
                } else {
                    pending.push(g);
                    let command = command_selection(&layout, &pending).is_some();
                    let pause = if command {
                        COMMAND_DEBOUNCE
                    } else if MATH_MODE.load(Ordering::Relaxed) {
                        MATH_DEBOUNCE
                    } else {
                        debounce
                    };
                    if command && pending.len() == 1 {
                        show_status(&mut surf, &disp, &font, "circle: write command");
                    }
                    dispatch_due = Some((Instant::now() + pause, false));
                    forced_waits = 0;
                    eprintln!(
                        "inktype: pen_up pending={} debounce_ms={} command={} input_revision={}",
                        pending.len(),
                        pause.as_millis(),
                        command,
                        input_revision
                    );
                }
            }
        }

        if let Some((x0, y0, x1, y1)) = dirty {
            if last_flush.elapsed() >= Duration::from_millis(if takeover { 8 } else { 30 }) {
                disp.update(x0, y0, x1 - x0 + 1, y1 - y0 + 1, true);
                dirty = None;
                last_flush = Instant::now();
            }
        }

        // Rat returns only locally cleaned program output. Qwen then lightly
        // presents that value and chooses the natural insertion point.
        while let Ok(reply) = run_rx.try_recv() {
            let chars: Vec<char> = text.chars().collect();
            let Some(insert_at) =
                find_source_end(&chars, reply.source_start, reply.source_end, &reply.source)
            else {
                eprintln!(
                    "inktype: discarded rat result={} selector={:?}; source changed",
                    reply.id, reply.selector
                );
                continue;
            };
            let (raw, error) = match reply.result {
                Ok(output) => (output, false),
                Err(error) => (error, true),
            };
            if let Some(oracle) = &oracle {
                placement_id += 1;
                oracle.place_result(
                    placement_id,
                    text_revision,
                    text.clone(),
                    reply.source_start,
                    reply.source_end,
                    reply.source,
                    reply.selector,
                    raw,
                    error,
                    place_tx.clone(),
                );
                paint_status(&mut surf, &font, "placing result");
                disp.update(STATUS_X as i32, 0, STATUS_W as i32, STATUS_H as i32, true);
            } else {
                insert_presented_result(
                    &mut text,
                    insert_at,
                    &raw,
                    &mut undo,
                    &mut redo,
                );
                text_revision += 1;
                let _ = std::fs::write(&path, &text);
                layout = render_all(&mut surf, &font, &text, &pending, scroll_y);
                disp.full_refresh(W, H);
            }
        }

        while let Ok(reply) = place_rx.try_recv() {
            let chars: Vec<char> = text.chars().collect();
            let fallback = find_source_end(
                &chars,
                reply.source_start,
                reply.source_end,
                &reply.source,
            );
            let Ok(placement) = reply.result else {
                eprintln!("inktype: result placement {} failed", reply.id);
                continue;
            };
            let at = if reply.text_revision == text_revision {
                placement.at
            } else if let Some(at) = fallback {
                at
            } else {
                eprintln!("inktype: discarded placement {}; source changed", reply.id);
                continue;
            };
            if at > chars.len() || placement.text.trim().is_empty() {
                continue;
            }
            insert_presented_result(
                &mut text,
                at,
                &placement.text,
                &mut undo,
                &mut redo,
            );
            text_revision += 1;
            if let Err(e) = std::fs::write(&path, &text) {
                eprintln!("inktype: save: {e}");
            }
            layout = render_all(&mut surf, &font, &text, &pending, scroll_y);
            paint_status(&mut surf, &font, "result added");
            disp.update(0, TOP - FONT_PX as i32, W as i32, H as i32, true);
        }

        // Drain every completed call. A response is applicable only if no
        // newer pen contact and no text edit happened since its snapshot.
        // Calls cannot be interrupted inside rustls, but stale results are
        // logically cancelled and never touch the document.
        while let Ok((id, request_input_revision, request_text_revision, result)) = rx.try_recv() {
            if id != latest_request
                || request_input_revision != input_revision
                || request_text_revision != text_revision
            {
                eprintln!(
                    "inktype: cancelled stale response={} latest={} input={}/{} text={}/{}",
                    id,
                    latest_request,
                    request_input_revision,
                    input_revision,
                    request_text_revision,
                    text_revision
                );
                continue;
            }
            // Legacy RUN is the paper shorthand for the py selector.
            let result = result.map(|decision| match decision {
                Decision::Run {
                    consume,
                    start,
                    end,
                } => Decision::Rat {
                    consume,
                    start,
                    end,
                    selector: "py".into(),
                },
                other => other,
            });
            match result {
                Ok(Decision::Wait) => {
                    show_status(&mut surf, &disp, &font, "AI: waiting for stroke");
                    if MATH_MODE.load(Ordering::Relaxed) && forced_waits < MAX_FORCED_WAITS {
                        forced_waits += 1;
                        dispatch_due = Some((Instant::now() + WAIT_RETRY, true));
                    } else {
                        // In text mode `?` is a true pass: retain every stroke
                        // and wait for new ink instead of retrying the same crop.
                        dispatch_due = None;
                        eprintln!("inktype: pass; retaining ink until another gesture");
                    }
                }
                Ok(Decision::Rat {
                    consume,
                    start: model_start,
                    end: model_end,
                    selector,
                }) => {
                    let Some((selected_start, selected_end)) = command_selection(&layout, &pending)
                    else {
                        eprintln!("inktype: refused rat command without a local circle selection");
                        continue;
                    };
                    let (start, end) = (selected_start, selected_end);
                    if (model_start, model_end) != (start, end) {
                        eprintln!(
                            "inktype: corrected model rat range={model_start}..{model_end} to local selection={start}..{end}"
                        );
                    }
                    let chars: Vec<char> = text.chars().collect();
                    if consume == 0 || start > end || end > chars.len() {
                        eprintln!("inktype: invalid rat range={start}..{end}");
                        continue;
                    }
                    let source: String = chars[start..end].iter().collect();
                    pending.clear();
                    dispatch_due = None;
                    forced_waits = 0;
                    caret = None;
                    input_revision += 1;
                    run_id += 1;
                    layout = render_all(&mut surf, &font, &text, &pending, scroll_y);
                    paint_status(&mut surf, &font, &format!("{selector}: running"));
                    disp.update(
                        0,
                        TOP - FONT_PX as i32,
                        W as i32,
                        H as i32 - TOP + FONT_PX as i32,
                        true,
                    );
                    eprintln!(
                        "inktype: rat run={run_id} selector={selector:?} range={start}..{end} source={source:?}"
                    );
                    runner.run(
                        run_id,
                        selector,
                        start,
                        end,
                        source.clone(),
                        source,
                        run_tx.clone(),
                    );
                }
                Ok(Decision::Data {
                    consume,
                    start: model_start,
                    end: model_end,
                    runtime,
                    name,
                    json,
                }) => {
                    let Some((start, end)) = command_selection(&layout, &pending) else {
                        eprintln!("inktype: refused data capture without circle selection");
                        continue;
                    };
                    if (model_start, model_end) != (start, end) {
                        eprintln!("inktype: corrected data range to {start}..{end}");
                    }
                    let chars: Vec<char> = text.chars().collect();
                    if consume == 0 || end > chars.len() {
                        continue;
                    }
                    let source: String = chars[start..end].iter().collect();
                    let execution = match repl::dataframe_code(&runtime, &name, &json) {
                        Ok(code) => code,
                        Err(error) => {
                            eprintln!("inktype: data capture: {error}");
                            continue;
                        }
                    };
                    pending.clear();
                    dispatch_due = None;
                    forced_waits = 0;
                    caret = None;
                    input_revision += 1;
                    run_id += 1;
                    layout = render_all(&mut surf, &font, &text, &pending, scroll_y);
                    paint_status(&mut surf, &font, &format!("{runtime}: making {name}"));
                    disp.update(0, TOP - FONT_PX as i32, W as i32, H as i32, true);
                    runner.run(
                        run_id,
                        runtime,
                        start,
                        end,
                        execution,
                        source,
                        run_tx.clone(),
                    );
                }
                Ok(Decision::Math {
                    consume,
                    start,
                    end,
                    latex,
                    semantic,
                }) => {
                    let mut chars: Vec<char> = text.chars().collect();
                    if consume <= pending.len() && start <= end && end <= chars.len() {
                        checkpoint(&mut undo, &mut redo, &text);
                        let payload = serde_json::json!({"latex": latex, "semantic": semantic});
                        let replacement = format!("[[INKTYPE_MATH:{payload}]]");
                        chars.splice(start..end, replacement.chars());
                        text = chars.into_iter().collect();
                        pending.drain(0..consume);
                        text_revision += 1;
                        forced_waits = 0;
                        dispatch_due = if pending.is_empty() {
                            None
                        } else {
                            Some((Instant::now() + debounce, false))
                        };
                        caret = None;
                        if let Err(e) = std::fs::write(&path, &text) {
                            eprintln!("inktype: save: {e}");
                        }
                        layout = render_all(&mut surf, &font, &text, &pending, scroll_y);
                        paint_status(&mut surf, &font, "math recognized");
                        disp.update(0, TOP - FONT_PX as i32, W as i32, H as i32, true);
                    }
                }
                Ok(Decision::MathCalculate { consume, .. }) => {
                    let Some((start, end)) = command_selection(&layout, &pending) else {
                        eprintln!("inktype: refused math calculation without circle selection");
                        continue;
                    };
                    let chars: Vec<char> = text.chars().collect();
                    let selected: String = chars[start..end].iter().collect();
                    let Some(payload) = extract_math_payload(&selected) else {
                        eprintln!("inktype: selected range contains no canonical math block");
                        continue;
                    };
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) else {
                        continue;
                    };
                    let Some(semantic) = value.get("semantic").and_then(serde_json::Value::as_str)
                    else {
                        continue;
                    };
                    let semantic_literal = serde_json::to_string(semantic).unwrap();
                    let execution = math_calculation_code(&semantic_literal);
                    // The circle and action word form one command unit. The
                    // local selection is authoritative, so consume all of it.
                    let _ = consume;
                    pending.clear();
                    dispatch_due = None;
                    forced_waits = 0;
                    caret = None;
                    input_revision += 1;
                    run_id += 1;
                    layout = render_all(&mut surf, &font, &text, &pending, scroll_y);
                    paint_status(&mut surf, &font, "math: calculating");
                    disp.update(0, TOP - FONT_PX as i32, W as i32, H as i32, true);
                    runner.run(
                        run_id,
                        "py".into(),
                        start,
                        end,
                        execution,
                        selected,
                        run_tx.clone(),
                    );
                }
                Ok(Decision::Edit {
                    consume,
                    start,
                    end,
                    text: replacement,
                }) => {
                    // Command gestures always apply to the locally computed
                    // circle selection; do not trust a vision model to count
                    // the character offsets itself.
                    let command_range = command_selection(&layout, &pending);
                    let (start, end) = command_range.unwrap_or((start, end));
                    let consume = if command_range.is_some() {
                        pending.len()
                    } else {
                        consume
                    };
                    if command_range.is_some() {
                        let action_leak = matches!(
                            replacement.trim().to_ascii_lowercase().as_str(),
                            "correct" | "sort" | "calculate" | "reflow" | "run" | "py" | "r"
                        );
                        if replacement.is_empty() || action_leak {
                            eprintln!(
                                "inktype: rejected destructive/leaked command replacement={replacement:?}"
                            );
                            show_status(&mut surf, &disp, &font, "command unclear - retained");
                            continue;
                        }
                    }
                    let mut chars: Vec<char> = text.chars().collect();
                    if consume <= pending.len() && start <= end && end <= chars.len() {
                        checkpoint(&mut undo, &mut redo, &text);
                        chars.splice(start..end, replacement.chars());
                        text = chars.into_iter().collect();
                        pending.drain(0..consume);
                        text_revision += 1;
                        forced_waits = 0;
                        dispatch_due = if pending.is_empty() {
                            None
                        } else {
                            Some((Instant::now() + debounce, false))
                        };
                        caret = None;
                        if let Err(e) = std::fs::write(&path, &text) {
                            eprintln!("inktype: save: {e}");
                        }
                        let paint = Instant::now();
                        layout = render_all(&mut surf, &font, &text, &pending, scroll_y);
                        paint_status(&mut surf, &font, "AI: recognized");
                        disp.update(
                            0,
                            TOP - FONT_PX as i32,
                            W as i32,
                            H as i32 - TOP + FONT_PX as i32,
                            true,
                        );
                        eprintln!("inktype: applied request={id} consume={consume} range={start}..{end} replacement={replacement:?} text_revision={text_revision} paint_ms={}", paint.elapsed().as_millis());
                    }
                }
                Ok(Decision::Run { .. }) => unreachable!("RUN normalized to RAT"),
                Err(e) => {
                    eprintln!("inktype: oracle error request={id}: {e}; retaining ink");
                    if command_selection(&layout, &pending).is_some() {
                        show_status(&mut surf, &disp, &font, "command unclear - retained");
                        dispatch_due = None;
                    } else if MATH_MODE.load(Ordering::Relaxed) && forced_waits < MAX_FORCED_WAITS {
                        show_status(&mut surf, &disp, &font, "AI error - retrying");
                        forced_waits += 1;
                        dispatch_due = Some((Instant::now() + ERROR_RETRY, true));
                    } else {
                        show_status(&mut surf, &disp, &font, "AI error - add stroke");
                        dispatch_due = None;
                    }
                }
            }
        }

        if current.is_none()
            && dispatch_due.is_some_and(|(when, _)| Instant::now() >= when)
            && !pending.is_empty()
        {
            let (_, force) = dispatch_due.take().unwrap();
            if let Some(id) = launch(
                &oracle,
                &mut surf,
                &layout,
                &text,
                &pending,
                caret,
                &rat_runtimes,
                force,
                input_revision,
                text_revision,
                &tx,
                &mut request_id,
            ) {
                latest_request = id;
                show_status(
                    &mut surf,
                    &disp,
                    &font,
                    &format!("recognizing {}", pending.len()),
                );
            }
        }

        let _ = disp.pump();
        std::thread::sleep(Duration::from_millis(2));
    }
    disp.terminate();
    eprintln!("inktype: closed");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn launch(
    oracle: &Option<oracle::Oracle>,
    surf: &mut Surface,
    layout: &Layout,
    text: &str,
    pending: &[Gesture],
    caret: Option<usize>,
    rat_runtimes: &str,
    force: bool,
    input_revision: u64,
    text_revision: u64,
    tx: &mpsc::Sender<oracle::Reply>,
    request_id: &mut u64,
) -> Option<u64> {
    let Some(oracle) = oracle else { return None };
    if pending.is_empty() {
        return None;
    }
    let t0 = Instant::now();
    let (x, y, w, h) = pending_crop(pending);
    let png = match crop_png(surf, x, y, w, h) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("inktype: encode: {e}");
            return None;
        }
    };
    *request_id += 1;
    let first = pending
        .iter()
        .find_map(|g| g.points.first())
        .copied()
        .unwrap_or((LEFT, TOP, 2));
    // A recent erase leaves a caret: new ink near it replaces the erased
    // text, so anchor there unless the writing is clearly on another line.
    let caret_anchor = caret.and_then(|c| {
        layout
            .insertions
            .iter()
            .find(|p| p.index == c)
            .filter(|p| (p.baseline - first.1).abs() <= LINE_STEP)
            .map(|p| p.index)
    });
    let selection = command_selection(layout, pending);
    let anchor = selection
        .map(|(start, _)| start)
        .or(caret_anchor)
        .unwrap_or_else(|| nearest_anchor(layout, first.0, first.1));
    let tool = "pen";
    let text_chars: Vec<char> = text.chars().collect();
    let line_map = layout
        .lines
        .iter()
        .map(|l| {
            let s: String = text_chars[l.start..l.end].iter().collect();
            format!("y={} chars {}..{} {:?}", l.baseline, l.start, l.end, s)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let last_baseline = layout.lines.last().map(|l| l.baseline).unwrap_or(TOP);
    let mut hint = String::new();
    let pending_count = pending.len();
    if let Some((start, end)) = selection {
        let selected: String = text_chars[start..end].iter().collect();
        hint.push_str(&format!(
            "COMMAND MODE. The first gesture is a loose circle selecting exact range {start}..{end}, SELECTED={selected:?}. Read the handwritten action beside the circle. Supported transformations: sort, calculate, correct prose/code (preserve fences), and reflow. 'df NAME', 'py df NAME', or 'r df NAME' captures the selected table as a persistent data frame (defaults: py and name df); return DATA:{pending_count},{start},{end}:RUNTIME:NAME:JSON with compact columns/rows JSON. A handwritten runtime selector executes the exact selection with rat: run means py; common selectors are py, r, and pi; named selectors are allowed. Known selectors: [{rat_runtimes}]. For execution return RAT:{pending_count},{start},{end}:SELECTOR. For transformations edit exactly {start}..{end}. Consume every pending gesture. If no action/selector is legible yet, return ?. "
        ));
    }
    if selection.is_none() {
        if let Some(c) = caret_anchor {
            hint.push_str(&format!(
                "CARET={c} (user just erased here; insert the new writing at this offset). "
            ));
        }
        if let Some(pos) = layout.insertions.iter().find(|pos| pos.index == anchor) {
            let gap = first.0 - pos.x;
            if gap > 34 && (first.1 - pos.baseline).abs() <= LINE_STEP / 2 {
                hint.push_str(&format!(
                    "GAP_AFTER_TEXT={gap}px: preserve this visible word boundary by prefixing the inserted text with one space unless TEXT already ends in whitespace. "
                ));
            }
        }
    }
    if selection.is_none() && first.1 > last_baseline + LINE_STEP / 2 {
        let below = ((first.1 - last_baseline) as f32 / LINE_STEP as f32).round() as usize;
        hint.push_str(&format!(
            "NEWLINE: ink starts {below} ruled line(s) below the last typeset line; T should begin with {} then the recognized text. ",
            "<NL>".repeat(below.max(1))
        ));
    }
    let gesture_summary = pending
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let first = g.points.first().copied().unwrap_or_default();
            let last = g.points.last().copied().unwrap_or_default();
            format!(
                "{}{}({},{})-({},{})",
                i + 1,
                if g.tool == Tool::Pen { 'p' } else { 'e' },
                first.0,
                first.1,
                last.0,
                last.1
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    let req = Request {
        id: *request_id,
        input_revision,
        text_revision,
        text: text.into(),
        anchor,
        pending_count: pending.len(),
        gesture_summary,
        tool,
        line_map,
        hint,
        command: selection.is_some(),
        math_mode: MATH_MODE.load(Ordering::Relaxed),
        crop_x: x,
        crop_y: y,
        crop_w: w,
        crop_h: h,
        png,
        force,
    };
    eprintln!(
        "inktype: dispatch={} pending={} anchor={} force={} input_revision={} text_revision={} prepare_ms={}",
        *request_id,
        pending.len(),
        anchor,
        force,
        input_revision,
        text_revision,
        t0.elapsed().as_millis()
    );
    oracle.ask(req, tx.clone());
    Some(*request_id)
}

const STATUS_X: usize = 1040;
const STATUS_W: usize = W - STATUS_X - 24;
const STATUS_H: usize = 62;

const RESET_X: i32 = 24;
const RESET_Y: i32 = 10;
const RESET_W: i32 = 170;
const RESET_H: i32 = 52;
const MODE_X: i32 = RESET_X + RESET_W + 18;
const MODE_Y: i32 = RESET_Y;
const MODE_W: i32 = 190;
const MODE_H: i32 = RESET_H;

fn in_reset_button(g: &Gesture) -> bool {
    !g.points.is_empty()
        && g.points.iter().all(|&(x, y, _)| {
            (RESET_X..=RESET_X + RESET_W).contains(&x) && (RESET_Y..=RESET_Y + RESET_H).contains(&y)
        })
}

fn in_mode_button(g: &Gesture) -> bool {
    !g.points.is_empty()
        && g.points.iter().all(|&(x, y, _)| {
            (MODE_X..=MODE_X + MODE_W).contains(&x)
                && (MODE_Y..=MODE_Y + MODE_H).contains(&y)
        })
}

fn math_calculate_hit(layout: &Layout, g: &Gesture) -> Option<(usize, usize)> {
    let &(x, y, _) = g.points.first()?;
    layout
        .math_calculates
        .iter()
        .find(|button| {
            (button.x - 12..=button.x + button.size + 12).contains(&x)
                && (button.y - 12..=button.y + button.size + 12).contains(&y)
        })
        .map(|button| (button.start, button.end))
}

fn output_close_hit(layout: &Layout, g: &Gesture) -> Option<(usize, usize)> {
    let &(x, y, _) = g.points.first()?;
    layout
        .output_closes
        .iter()
        .find(|close| {
            (close.x - 12..=close.x + close.size + 12).contains(&x)
                && (close.y - 12..=close.y + close.size + 12).contains(&y)
        })
        .map(|close| (close.start, close.end))
}

fn paint_mode_button(surf: &mut Surface, font: &FontRef) {
    let (x, y, w, h) = (MODE_X as usize, MODE_Y as usize, MODE_W as usize, MODE_H as usize);
    surf.fill_rect(x, y, w, h, WHITE);
    let color = BLACK;
    surf.fill_rect(x, y, w, 2, color);
    surf.fill_rect(x, y + h - 2, w, 2, color);
    surf.fill_rect(x, y, 2, h, color);
    surf.fill_rect(x + w - 2, y, 2, h, color);
    draw_label(
        surf,
        font,
        27.0,
        x as f32 + 22.0,
        y as f32 + 36.0,
        if MATH_MODE.load(Ordering::Relaxed) { "Math: ON" } else { "Math: OFF" },
    );
}

fn paint_reset_button(surf: &mut Surface, font: &FontRef) {
    let (x, y, w, h) = (
        RESET_X as usize,
        RESET_Y as usize,
        RESET_W as usize,
        RESET_H as usize,
    );
    surf.fill_rect(x, y, w, h, WHITE);
    surf.fill_rect(x, y, w, 2, BLACK);
    surf.fill_rect(x, y + h - 2, w, 2, BLACK);
    surf.fill_rect(x, y, 2, h, BLACK);
    surf.fill_rect(x + w - 2, y, 2, h, BLACK);
    draw_label(surf, font, 27.0, x as f32 + 38.0, y as f32 + 36.0, "Reset");
}

fn draw_label(surf: &mut Surface, font: &FontRef, px: f32, mut x: f32, baseline: f32, text: &str) {
    let scaled = font.as_scaled(PxScale::from(px));
    let mut prev = None;
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(p) = prev {
            x += scaled.kern(p, id);
        }
        let mut glyph: Glyph = id.with_scale(PxScale::from(px));
        glyph.position = ab_glyph::point(x, baseline);
        if let Some(outline) = font.outline_glyph(glyph) {
            let b = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                if cov > 0.42 {
                    surf.put_px(
                        b.min.x as i32 + gx as i32,
                        b.min.y as i32 + gy as i32,
                        BLACK,
                    );
                }
            });
        }
        x += scaled.h_advance(id);
        prev = Some(id);
    }
}

fn show_status(surf: &mut Surface, disp: &display::Display, font: &FontRef, text: &str) {
    paint_status(surf, font, text);
    disp.update(STATUS_X as i32, 0, STATUS_W as i32, STATUS_H as i32, true);
}

fn paint_status(surf: &mut Surface, font: &FontRef, text: &str) {
    surf.fill_rect(STATUS_X, 0, STATUS_W, STATUS_H, WHITE);
    let px = 27.0;
    let scaled = font.as_scaled(PxScale::from(px));
    let mut x = STATUS_X as f32;
    let mut prev = None;
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(p) = prev {
            x += scaled.kern(p, id);
        }
        let mut glyph: Glyph = id.with_scale(PxScale::from(px));
        glyph.position = ab_glyph::point(x, 39.0);
        if let Some(outline) = font.outline_glyph(glyph) {
            let b = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                if cov > 0.42 {
                    surf.put_px(
                        b.min.x as i32 + gx as i32,
                        b.min.y as i32 + gy as i32,
                        BLACK,
                    );
                }
            });
        }
        x += scaled.h_advance(id);
        if x >= (STATUS_X + STATUS_W) as f32 {
            break;
        }
        prev = Some(id);
    }
}

fn render_all(
    surf: &mut Surface,
    font: &FontRef,
    text: &str,
    pending: &[Gesture],
    scroll_y: i32,
) -> Layout {
    surf.fill_rect(0, 0, W, H, WHITE);
    // Quiet ruled paper. Rules are deliberately light enough not to interfere
    // with OCR but remain visible on the color/grayscale framebuffer.
    let rule = 0xDEFB;
    let mut y = TOP + RULE_OFFSET;
    while y < H as i32 {
        surf.fill_rect(55, y as usize, W - 110, 2, rule);
        y += LINE_STEP;
    }
    let layout = draw_document(surf, font, text, scroll_y);
    for g in pending {
        draw_gesture(surf, g);
    }
    paint_reset_button(surf, font);
    paint_mode_button(surf, font);
    layout
}

fn draw_document(surf: &mut Surface, font: &FontRef, text: &str, scroll_y: i32) -> Layout {
    let chars: Vec<char> = text.chars().collect();
    let outputs = find_output_ranges(&chars);
    let images = find_image_ranges(&chars);
    let maths = find_math_ranges(&chars);
    let is_output = |index: usize| {
        outputs
            .iter()
            .any(|range| index >= range.start && index < range.end)
    };
    let mut x = LEFT as f32;
    let mut baseline = (TOP - scroll_y) as f32;
    let mut prev = None;
    let mut prev_output = false;
    let mut insertions = vec![Pos {
        index: 0,
        x: LEFT,
        baseline: TOP - scroll_y,
    }];
    let mut cells: Vec<Cell> = Vec::new();
    let mut lines: Vec<Line> = Vec::new();
    let mut line_start = 0usize;
    let mut math_cell_x1 = LEFT + 300;
    let mut math_cell_baseline = baseline as i32;
    for (i, &ch) in chars.iter().enumerate() {
        if let Some(math) = maths.iter().find(|math| i >= math.start && i < math.end) {
            if i == math.start {
                x = LEFT as f32;
                let top = baseline as i32 - 45;
                let rendered = render_math_png(&math.payload);
                math_cell_x1 = rendered
                    .as_deref()
                    .and_then(png_display_width)
                    .map(|width| LEFT + width)
                    .unwrap_or(LEFT + 300);
                let height = rendered
                    .and_then(|path| draw_png_block(surf, &path, LEFT, top))
                    .unwrap_or_else(|| {
                        draw_label(surf, font, 32.0, LEFT as f32, baseline, "[math]");
                        80
                    });
                math_cell_baseline = top + height / 2 + FONT_PX as i32 / 3;
                baseline += height as f32;
            }
            prev = None;
            // All marker characters share the actual rendered equation box,
            // making a loose circle select the complete canonical block.
            cells.push(Cell {
                index: i,
                x0: LEFT,
                x1: math_cell_x1,
                baseline: math_cell_baseline,
            });
            insertions.push(Pos {
                index: i + 1,
                x: LEFT,
                baseline: baseline as i32,
            });
            continue;
        }
        if let Some(image) = images
            .iter()
            .find(|image| i >= image.start && i < image.end)
        {
            if i == image.start {
                x = LEFT as f32;
                let top = baseline as i32 - 28;
                let height = draw_png_block(surf, &image.path, LEFT, top).unwrap_or(120);
                baseline += height as f32;
            }
            prev = None;
            cells.push(Cell {
                index: i,
                x0: LEFT,
                x1: LEFT,
                baseline: baseline as i32,
            });
            insertions.push(Pos {
                index: i + 1,
                x: LEFT,
                baseline: baseline as i32,
            });
            continue;
        }
        let output = is_output(i);
        let px = if output { OUTPUT_FONT_PX } else { FONT_PX };
        let scaled = font.as_scaled(PxScale::from(px));
        if output != prev_output {
            prev = None;
        }
        if ch == '\n' {
            lines.push(Line {
                baseline: baseline as i32,
                start: line_start,
                end: i,
            });
            line_start = i + 1;
            x = LEFT as f32;
            baseline += if is_output(i + 1) {
                OUTPUT_LINE_STEP as f32
            } else {
                LINE_STEP as f32
            };
            prev = None;
            prev_output = is_output(i + 1);
            insertions.push(Pos {
                index: i + 1,
                x: x as i32,
                baseline: baseline as i32,
            });
            continue;
        }
        let id = scaled.glyph_id(ch);
        if let Some(p) = prev {
            x += scaled.kern(p, id);
        }
        let advance = scaled.h_advance(id);
        if x + advance > (W as i32 - RIGHT) as f32 && x > LEFT as f32 {
            lines.push(Line {
                baseline: baseline as i32,
                start: line_start,
                end: i,
            });
            line_start = i;
            x = LEFT as f32;
            baseline += if output {
                OUTPUT_LINE_STEP as f32
            } else {
                LINE_STEP as f32
            };
        }
        let cell_x0 = x as i32;
        if baseline > -px && baseline < H as f32 + px {
            let mut glyph: Glyph = id.with_scale(PxScale::from(px));
            glyph.position = ab_glyph::point(x, baseline);
            if let Some(outline) = font.outline_glyph(glyph) {
                let b = outline.px_bounds();
                outline.draw(|gx, gy, cov| {
                    if cov > 0.42 {
                        surf.put_px(
                            b.min.x as i32 + gx as i32,
                            b.min.y as i32 + gy as i32,
                            BLACK,
                        );
                    }
                });
            }
        }
        x += advance;
        prev = Some(id);
        prev_output = output;
        cells.push(Cell {
            index: i,
            x0: cell_x0,
            x1: x as i32,
            baseline: baseline as i32,
        });
        insertions.push(Pos {
            index: i + 1,
            x: x as i32,
            baseline: baseline as i32,
        });
    }
    lines.push(Line {
        baseline: baseline as i32,
        start: line_start,
        end: chars.len(),
    });

    let mut output_closes = Vec::new();
    let mut math_calculates = Vec::new();
    for output in outputs {
        if let Some(pos) = insertions.iter().find(|pos| pos.index == output.start) {
            let size = 38;
            let x = W as i32 - RIGHT - size;
            let y = pos.baseline - size + 7;
            if y + size >= 0 && y < H as i32 {
                surf.fill_rect(x as usize, y.max(0) as usize, size as usize, 2, BLACK);
                surf.fill_rect(
                    x as usize,
                    (y + size - 2).max(0) as usize,
                    size as usize,
                    2,
                    BLACK,
                );
                surf.fill_rect(x as usize, y.max(0) as usize, 2, size as usize, BLACK);
                surf.fill_rect(
                    (x + size - 2) as usize,
                    y.max(0) as usize,
                    2,
                    size as usize,
                    BLACK,
                );
                draw_label(surf, font, 27.0, x as f32 + 10.0, y as f32 + 28.0, "×");
            }
            output_closes.push(OutputClose {
                x,
                y,
                size,
                start: output.delete_start,
                end: output.delete_end,
            });
        }
    }
    for math in &maths {
        if let Some(pos) = insertions.iter().find(|pos| pos.index == math.start) {
            let size = 46;
            let calculate = MathCalculate {
                x: W as i32 - RIGHT - size * 2 - 16,
                y: pos.baseline - 20,
                size,
                start: math.start,
                end: math.end,
            };
            if calculate.y + size >= 0 && calculate.y < H as i32 {
                draw_close_button(surf, font, calculate.x, calculate.y, size);
                surf.fill_rect(calculate.x as usize + 5, calculate.y.max(0) as usize + 5, (size - 10) as usize, (size - 10) as usize, WHITE);
                draw_label(surf, font, 27.0, calculate.x as f32 + 11.0, calculate.y as f32 + 31.0, "▶");
            }
            math_calculates.push(calculate);
            output_closes.push(OutputClose {
                x: W as i32 - RIGHT - 46,
                y: pos.baseline - 20,
                size: 46,
                start: math.delete_start,
                end: math.delete_end,
            });
            let close = output_closes.last().unwrap();
            if close.y + close.size >= 0 && close.y < H as i32 {
                draw_close_button(surf, font, close.x, close.y, close.size);
            }
        }
    }
    for image in &images {
        if let Some(pos) = insertions.iter().find(|pos| pos.index == image.start) {
            output_closes.push(OutputClose {
                x: W as i32 - RIGHT - 46,
                y: pos.baseline - 20,
                size: 46,
                start: image.delete_start,
                end: image.delete_end,
            });
            let close = output_closes.last().unwrap();
            if close.y + close.size >= 0 && close.y < H as i32 {
                draw_close_button(surf, font, close.x, close.y, close.size);
            }
        }
    }
    let content_height = baseline as i32 + scroll_y + LINE_STEP;
    Layout {
        insertions,
        cells,
        lines,
        output_closes,
        math_calculates,
        content_height,
    }
}

fn find_output_ranges(chars: &[char]) -> Vec<OutputRange> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for i in 0..=chars.len() {
        if i == chars.len() || chars[i] == '\n' {
            lines.push((start, i));
            start = i + 1;
        }
    }
    let mut outputs = Vec::new();
    let mut line = 0usize;
    while line < lines.len() {
        let (start, mut end) = lines[line];
        let value: String = chars[start..end].iter().collect();
        if is_rat_output_header(&value) {
            let mut last_line = line;
            while last_line + 1 < lines.len() {
                let (next_start, next_end) = lines[last_line + 1];
                let next: String = chars[next_start..next_end].iter().collect();
                if !next.starts_with("  ") {
                    break;
                }
                last_line += 1;
                end = next_end;
            }
            outputs.push(OutputRange {
                start,
                end,
                delete_start: if start > 0 && chars[start - 1] == '\n' {
                    start - 1
                } else {
                    start
                },
                delete_end: end,
            });
            line = last_line + 1;
        } else {
            line += 1;
        }
    }
    outputs
}

fn find_image_ranges(chars: &[char]) -> Vec<ImageRange> {
    let mut images = Vec::new();
    let mut start = 0usize;
    for end in 0..=chars.len() {
        if end == chars.len() || chars[end] == '\n' {
            let line: String = chars[start..end].iter().collect();
            if let Some(path) = line
                .strip_prefix("[[INKTYPE_IMAGE:")
                .and_then(|value| value.strip_suffix("]]"))
            {
                images.push(ImageRange {
                    start,
                    end,
                    delete_start: if start > 0 && chars[start - 1] == '\n' {
                        start - 1
                    } else {
                        start
                    },
                    delete_end: end,
                    path: path.to_string(),
                });
            }
            start = end + 1;
        }
    }
    images
}

fn math_calculation_code(semantic_literal: &str) -> String {
    format!(
        "import json, sympy as sp\nexpr = sp.sympify({semantic_literal}, locals=vars(sp))\nresult = sp.simplify(expr.doit())\nprint('__INKTYPE_MATH__:' + json.dumps({{'latex': sp.latex(result), 'semantic': sp.srepr(result)}}, separators=(',', ':')))"
    )
}

fn extract_math_payload(text: &str) -> Option<String> {
    let start = text.find("[[INKTYPE_MATH:")? + "[[INKTYPE_MATH:".len();
    let end = text[start..].find("]]")? + start;
    let payload = &text[start..end];
    serde_json::from_str::<serde_json::Value>(payload).ok()?;
    Some(payload.to_string())
}

fn find_math_ranges(chars: &[char]) -> Vec<MathRange> {
    let mut maths = Vec::new();
    let mut start = 0usize;
    for end in 0..=chars.len() {
        if end == chars.len() || chars[end] == '\n' {
            let line: String = chars[start..end].iter().collect();
            if let Some(payload) = line
                .strip_prefix("[[INKTYPE_MATH:")
                .and_then(|v| v.strip_suffix("]]"))
            {
                if serde_json::from_str::<serde_json::Value>(payload).is_ok() {
                    maths.push(MathRange {
                        start,
                        end,
                        delete_start: if start > 0 && chars[start - 1] == '\n' {
                            start - 1
                        } else {
                            start
                        },
                        delete_end: end,
                        payload: payload.to_string(),
                    });
                }
            }
            start = end + 1;
        }
    }
    maths
}

fn render_math_png(payload: &str) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let latex = value.get("latex")?.as_str()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    latex.hash(&mut hasher);
    let dir = "/home/root/inktype-assets";
    std::fs::create_dir_all(dir).ok()?;
    let path = format!("{dir}/math-{:016x}.png", hasher.finish());
    if std::path::Path::new(&path).exists() {
        return Some(path);
    }
    let script = r#"import sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
latex, path = sys.argv[1], sys.argv[2]
fig = plt.figure(figsize=(0.01, 0.01))
fig.patch.set_alpha(0)
t = fig.text(0, 0, '$' + latex + '$', fontsize=34, color='black')
fig.canvas.draw()
b = t.get_window_extent().expanded(1.08, 1.25)
fig.set_size_inches(b.width / fig.dpi, b.height / fig.dpi)
t.set_position((0.04, 0.15))
fig.savefig(path, dpi=fig.dpi, transparent=False, facecolor='white', bbox_inches='tight', pad_inches=0.08)
"#;
    let status = std::process::Command::new("python")
        .args(["-c", script, latex, &path])
        .status()
        .ok()?;
    status.success().then_some(path)
}

fn draw_close_button(surf: &mut Surface, font: &FontRef, x: i32, y: i32, size: i32) {
    if x < 0 || y + size < 0 || y >= H as i32 {
        return;
    }
    let top = y.max(0) as usize;
    surf.fill_rect(x as usize, top, size as usize, 2, BLACK);
    surf.fill_rect(
        x as usize,
        (y + size - 2).max(0) as usize,
        size as usize,
        2,
        BLACK,
    );
    surf.fill_rect(x as usize, top, 2, size as usize, BLACK);
    surf.fill_rect((x + size - 2) as usize, top, 2, size as usize, BLACK);
    draw_label(
        surf,
        font,
        27.0,
        x as f32 + size as f32 / 3.5,
        y as f32 + size as f32 * 0.72,
        "×",
    );
}

fn png_display_width(path: &str) -> Option<i32> {
    let file = std::fs::File::open(path).ok()?;
    let reader = png::Decoder::new(file).read_info().ok()?;
    let info = reader.info();
    let max_w = (W as i32 - LEFT - RIGHT) as usize;
    let max_h = 850usize;
    let scale = (max_w as f32 / info.width as f32)
        .min(max_h as f32 / info.height as f32)
        .min(1.0);
    Some((info.width as f32 * scale).max(1.0) as i32)
}

fn draw_png_block(surf: &mut Surface, path: &str, x: i32, y: i32) -> Option<i32> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut data = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut data).ok()?;
    let (source_w, source_h) = (info.width as usize, info.height as usize);
    let max_w = (W as i32 - LEFT - RIGHT) as usize;
    let max_h = 850usize;
    let scale = (max_w as f32 / source_w as f32)
        .min(max_h as f32 / source_h as f32)
        .min(1.0);
    let target_w = (source_w as f32 * scale).max(1.0) as usize;
    let target_h = (source_h as f32 * scale).max(1.0) as usize;
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Indexed => return None,
    };
    for ty in 0..target_h {
        let sy = ty * source_h / target_h;
        for tx in 0..target_w {
            let sx = tx * source_w / target_w;
            let offset = (sy * source_w + sx) * channels;
            let (r, g, b) = match info.color_type {
                png::ColorType::Rgb | png::ColorType::Rgba => {
                    (data[offset], data[offset + 1], data[offset + 2])
                }
                _ => (data[offset], data[offset], data[offset]),
            };
            let rgb565 = ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3);
            surf.put_px(x + tx as i32, y + ty as i32, rgb565);
        }
    }
    Some(target_h as i32 + 24)
}

fn is_rat_output_header(line: &str) -> bool {
    [" → ", " ! "].iter().any(|marker| {
        line.split_once(marker)
            .is_some_and(|(selector, _)| repl::valid_selector(selector))
    })
}

/// Thin insertion bar marking where the last erase happened.
fn draw_caret(surf: &mut Surface, layout: &Layout, index: usize) {
    if let Some(p) = layout.insertions.iter().find(|p| p.index == index) {
        let y0 = (p.baseline - FONT_PX as i32 + 8).max(0);
        surf.fill_rect(p.x.max(1) as usize, y0 as usize, 3, FONT_PX as usize, BLACK);
    }
}

/// Character range [start, end) of typeset glyphs swept by an eraser gesture.
fn erased_range(layout: &Layout, g: &Gesture) -> Option<(usize, usize)> {
    let ascent = FONT_PX as i32;
    let descent = 20;
    let mut lo = usize::MAX;
    let mut hi = 0usize;
    for c in &layout.cells {
        let (y0, y1) = (c.baseline - ascent, c.baseline + descent);
        let hit = g
            .points
            .iter()
            .any(|&(x, y, r)| x + r >= c.x0 && x - r <= c.x1 && y >= y0 && y <= y1);
        if hit {
            lo = lo.min(c.index);
            hi = hi.max(c.index + 1);
        }
    }
    (lo < hi).then_some((lo, hi))
}

/// Remove pending pen points covered by an eraser gesture, splitting the
/// surviving points into separate gestures so redrawn ink keeps the gap.
fn erase_pending_ink(pending: &mut Vec<Gesture>, eraser: &Gesture) {
    let mut out: Vec<Gesture> = Vec::new();
    for g in pending.drain(..) {
        let mut run: Vec<(i32, i32, i32)> = Vec::new();
        for &(x, y, r) in &g.points {
            let hit = eraser.points.iter().any(|&(ex, ey, er)| {
                let dx = (ex - x) as i64;
                let dy = (ey - y) as i64;
                let rr = (er + r + 2) as i64;
                dx * dx + dy * dy <= rr * rr
            });
            if hit {
                if run.len() > 1 {
                    out.push(Gesture {
                        tool: g.tool,
                        points: std::mem::take(&mut run),
                    });
                } else {
                    run.clear();
                }
            } else {
                run.push((x, y, r));
            }
        }
        if !run.is_empty() {
            out.push(Gesture {
                tool: g.tool,
                points: run,
            });
        }
    }
    *pending = out;
}

fn draw_gesture(surf: &mut Surface, g: &Gesture) {
    let color = if g.tool == Tool::Pen { BLACK } else { WHITE };
    for (i, &(x, y, r)) in g.points.iter().enumerate() {
        if i == 0 {
            surf.stamp(x, y, r, color);
        } else {
            let (px, py, pr) = g.points[i - 1];
            surf.brush_line(px, py, x, y, r.min(pr + 1), color);
        }
    }
}

/// Detect a large closed first stroke and return the typeset character range
/// enclosed by its loose bounding box. Small handwritten loops (o, e, etc.)
/// are deliberately excluded.
fn command_selection(layout: &Layout, pending: &[Gesture]) -> Option<(usize, usize)> {
    let g = pending.first()?;
    if g.tool != Tool::Pen || g.points.len() < 6 {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for &(x, y, _) in &g.points {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let (w, h) = (x1 - x0, y1 - y0);
    if w < 55 || h < 32 {
        return None;
    }
    let first = g.points.first()?;
    let last = g.points.last()?;
    let dx = (first.0 - last.0) as i64;
    let dy = (first.1 - last.1) as i64;
    // Loose hand-drawn circles often end well short of their starting point.
    // Command loops are often drawn quickly and may end far from their exact
    // starting point. Enclosed canonical glyphs provide the stronger signal.
    let close = ((w.max(h) as f32) * 0.90).max(80.0) as i64;
    if dx * dx + dy * dy > close * close {
        return None;
    }

    // Inset only slightly: users tend to draw circles through the edges of
    // glyphs. Character centers are stable even for loose, uneven loops.
    let mut lo = usize::MAX;
    let mut hi = 0usize;
    for c in &layout.cells {
        let cx = (c.x0 + c.x1) / 2;
        let cy = c.baseline - FONT_PX as i32 / 3;
        if cx >= x0 - 20 && cx <= x1 + 20 && cy >= y0 - 20 && cy <= y1 + 20 {
            lo = lo.min(c.index);
            hi = hi.max(c.index + 1);
        }
    }
    (lo < hi).then_some((lo, hi))
}

fn nearest_anchor(layout: &Layout, x: i32, y: i32) -> usize {
    // Vertical distance dominates: an insertion point on the right ruled
    // line is nearly always the intended one, even when it is far away in x.
    layout
        .insertions
        .iter()
        .min_by_key(|p| {
            let dx = (p.x - x) as i64;
            let dy = (p.baseline - y) as i64;
            dx * dx + dy * dy * 16
        })
        .map(|p| p.index)
        .unwrap_or(0)
}

fn pending_crop(pending: &[Gesture]) -> (i32, i32, usize, usize) {
    let (mut x0, mut y0, mut x1, mut y1) = (W as i32, H as i32, 0, 0);
    for g in pending {
        for &(x, y, r) in &g.points {
            x0 = x0.min(x - r);
            y0 = y0.min(y - r);
            x1 = x1.max(x + r);
            y1 = y1.max(y + r);
        }
    }
    let margin = 180;
    x0 = (x0 - margin).max(0);
    y0 = (y0 - margin).max(0);
    x1 = (x1 + margin).min(W as i32 - 1);
    y1 = (y1 + margin).min(H as i32 - 1);
    (x0, y0, (x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize)
}

fn crop_png(surf: &Surface, x0: i32, y0: i32, w: usize, h: usize) -> std::io::Result<Vec<u8>> {
    let factor = w.max(h).div_ceil(720).max(1);
    let ow = w.div_ceil(factor);
    let oh = h.div_ceil(factor);
    let mut gray = vec![255u8; ow * oh];
    for oy in 0..oh {
        for ox in 0..ow {
            let mut sum = 0u32;
            let mut n = 0u32;
            for dy in 0..factor {
                for dx in 0..factor {
                    let sx = ox * factor + dx;
                    let sy = oy * factor + dy;
                    if sx < w && sy < h {
                        sum += surf.luma(x0 + sx as i32, y0 + sy as i32) as u32;
                        n += 1;
                    }
                }
            }
            gray[oy * ow + ox] = (sum / n.max(1)) as u8;
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, ow as u32, oh as u32);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        enc.write_header()
            .map_err(std::io::Error::other)?
            .write_image_data(&gray)
            .map_err(std::io::Error::other)?;
    }
    Ok(out)
}

fn checkpoint(undo: &mut Vec<String>, redo: &mut Vec<String>, text: &str) {
    if undo.last().is_none_or(|last| last != text) {
        undo.push(text.to_string());
        if undo.len() > 100 {
            undo.remove(0);
        }
    }
    redo.clear();
}

fn find_source_end(
    chars: &[char],
    original_start: usize,
    original_end: usize,
    source: &str,
) -> Option<usize> {
    let source: Vec<char> = source.chars().collect();
    if original_start <= original_end
        && original_end <= chars.len()
        && chars[original_start..original_end] == source
    {
        return Some(original_end);
    }
    if source.is_empty() || source.len() > chars.len() {
        return None;
    }
    let matches: Vec<usize> = chars
        .windows(source.len())
        .enumerate()
        .filter_map(|(start, window)| (window == source).then_some(start + source.len()))
        .take(2)
        .collect();
    (matches.len() == 1).then_some(matches[0])
}

fn insert_presented_result(
    document: &mut String,
    at: usize,
    result: &str,
    undo: &mut Vec<String>,
    redo: &mut Vec<String>,
) {
    checkpoint(undo, redo, document);
    let mut chars: Vec<char> = document.chars().collect();
    chars.splice(at..at, result.chars());
    *document = chars.into_iter().collect();
}

#[allow(dead_code)]
fn format_repl_output(selector: &str, text: &str, error: bool) -> String {
    const MAX_CHARS: usize = 3000;
    let mut images = Vec::new();
    let plain = text
        .lines()
        .filter_map(|line| {
            if line.trim().starts_with("[[INKTYPE_IMAGE:")
                || line.trim().starts_with("[[INKTYPE_MATH:")
            {
                images.push(line.trim().to_string());
                None
            } else {
                Some(line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut body: String = plain.chars().take(MAX_CHARS).collect();
    if plain.chars().count() > MAX_CHARS {
        body.push_str("\n… output truncated");
    }
    let marker = if error { "!" } else { "→" };
    let mut blocks = Vec::new();
    if !body.trim().is_empty() || images.is_empty() {
        if body.trim().is_empty() {
            body = "(no output)".into();
        }
        blocks.push(format!(
            "{selector} {marker} {}",
            body.trim().replace('\n', "\n  ")
        ));
    }
    blocks.extend(images);
    blocks.join("\n")
}

fn dirty_add(d: &mut Option<(i32, i32, i32, i32)>, x: i32, y: i32, m: i32) {
    let p = (
        (x - m).max(0),
        (y - m).max(0),
        (x + m).min(W as i32 - 1),
        (y + m).min(H as i32 - 1),
    );
    *d = Some(match *d {
        None => p,
        Some((x0, y0, x1, y1)) => (x0.min(p.0), y0.min(p.1), x1.max(p.2), y1.max(p.3)),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_multiline_repl_output_as_one_deletable_block() {
        let chars: Vec<char> = "x = 1\nmy-py-here → 1\n  second line\nnext"
            .chars()
            .collect();
        let outputs = find_output_ranges(&chars);
        assert_eq!(outputs.len(), 1);
        let output = outputs[0];
        assert_eq!(
            chars[output.start..output.end].iter().collect::<String>(),
            "my-py-here → 1\n  second line"
        );
        assert_eq!(chars[output.delete_start], '\n');
    }
}
