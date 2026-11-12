# Agent rules

Rules for coding agents working in this repository (and good practice for anyone). They exist because
auto-commits and headless router runs both cost time.  They apply to every change, including documentation-only
ones and including this file.

## 1. Never commit on your own initiative

- Do **not** run `git commit`, `git commit --amend`, `git rebase`, `git reset`, `git tag`, `git stash` or
  `git push` unless the person you work for asks for it in that very message.  Make the change, run the checks,
  then **report**.
- End the turn by saying which files you touched and which are still modified, so that the diff can be reviewed and
  exactly the wanted changes committed (`git status --short`, `git diff --stat`).

## 2. Always run the router with the live view

- Always `uv run tools/route.py --live` (or `--live --pairs-only`) from `hardware/`.  Never without `--live`.  Every
  Python script runs with `uv run` (DEVELOPMENT.md, "Python scripts"); for a log written to a file, `uv run python -u
  tools/route.py --live > ...`.
- It is the only way to see *why* a lane failed or a pair was squeezed; the console log alone hides it, and that
  has repeatedly sent this project down the wrong path.  The flag is additive — it does not change the routing
  result — so use it for every run, including quick iterations.
- The viewer is `tools/route_view.html`, served with `tools/build/live/state.json` (tracks, vias, the router's log)
  and `static.json` (pads, outline) at **http://localhost:8765/**.  Wheel = zoom, drag = pan, **Fit board** resets,
  the net box (or clicking copper) highlights a net's pads, the layer switches toggle F.Cu / B.Cu / In2.Cu /
  In1.Cu, and the router's own `PAIR FAIL` lines show in the log panel.
- 8765 is **the** port: that is where the viewer is watched.  If something already holds it, **stop the old server
  first** (`fuser -k -TERM 8765/tcp`, then wait a moment) rather than moving to another port — moving leaves the
  open viewer dead.  `--live=PORT` exists only for a deliberate second viewer.
- For a slow run: start it in the background and poll — and poll the *process*
  (`pgrep -af '[v]env/bin/python.* tools/route.py'`: the interpreter itself, not `uv run`'s parent process), not a
  pattern that also matches the polling shell itself.
- When a result needs review, leave a browsable snapshot behind (copy `index.html`, `state.json` and `static.json`
  out of `tools/build/live/`, as `hardware/debug/swap-attempt/` does) and keep `tools/route.log` for the numbers.

## 3. Verify, then report *(follows from the two rules above)*

- The router is **not bit-deterministic**: identical runs differ by ~1 % in segment/via count, and a hard net can
  be left unrouted in one run and route fine in the next.  After every routing run, run
  `kicad-cli pcb drc --schematic-parity`, `uv run tools/check_copper.py`, `check_spacing.py` and `check_aux.py`.
  If a re-route disagrees with the committed board, keep the board that passes and say so.
- Quote measured numbers (`tools/route.log`, the DRC report, `check_copper.py`) instead of asserting that
  something is fine, and say which checks are calibrated: `report.md` §10 and the header of
  `tools/check_copper.py` list what is trustworthy (clearance on tracks/vias, stubs, pair metrics) and what is
  not yet (net-piece counting).
- `hardware/` is generated: edit the generators in `hardware/tools/`, never the `.kicad_sch` / `.kicad_pcb` by
  hand, and re-run the chain documented in `hardware/README.md`.
