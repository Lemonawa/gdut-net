# Finish review — GDUT Net installer + GUI

> Substitution disclosure: this harness could not load the shipped finish-reviewer agent, so the
> degraded reference (`reference/degraded/finish-reviewer.md`) was followed inline, in two passes:
> a review round over the recaptured evidence, then a verdict pass over the post-fix recaptures.
> Inputs not provided: no QUALITY BAR card path was supplied; no detector ran (native, code-led build).

Review round: `disposition: fix`

## persistence

Pass. `PRODUCT.md` exists at the root; the world is new, so `DESIGN.md` is written after this review
by the documenter (its absence in the review round is not a finding). This is a code-led build: no
comp round, no `.impeccable/mocks/` comps; `.impeccable/surfaces/src-tray-gui-rs.md` and
`.impeccable/surfaces/src-setup-ui-rs.md` carry both directions and their `FORM` seed key
`f8e9de4f` is corroborated.

## fidelity

No approved comp (code-led): TYPE and MATERIAL judged against OWN-WORLD; GROUND sampled from the
captures. Evidence: `.impeccable/review/gui.png`, `gui-service-down.png`, `setup-maintenance.png`.

Element matrix:

- 卡面（卡蓝地、圆角、读卡灯、状态章）— match (gui.png: #1D4E9E fills the card face; the reader
  light is the only circle; the stamp is re-pressed on status change per `src/tray/gui.rs`).
- 小票（票纸白、虚线分节、等宽数字、事件流水）— match (gui.png ground sampled #F4F1E8; field
  rows and events use the monospace family).
- 软键排（底部三键：修改账号密码 / 打开日志目录 / 版本）— match.
- 服务未运行页（朱红大字 + 启动服务 + 日志入口）— match (gui-service-down.png: #C2402F word,
  primary action, persistent soft keys).
- 安装器维护页 — contradicted in the review round: the page rendered as a plain egui-gray field
  (`#F8F8F8` sampled across 87% of the window) with no card face and no receipt frame, and the
  setup surface's palette constants were near-miss hexes (`#1B4F9C/#FBFAF5/#1F2328/#2E9E5B/#C0392B`)
  against the OWN-WORLD values.
- TYPE — adaptation: system CJK fonts (Deng/simhei/msyh/simsun) are forced by the product truth
  (Chinese Windows, zero font shipping); the surface briefs record this constraint.
- MATERIAL — match: flat card/receipt material is what OWN-WORLD promises; no fake physicality.
- GROUND — the tray surface samples exactly #F4F1E8/#1D4E9E; the setup surface was cool-gray drift.

## ceiling

Reached at this scope. The world's native devices are all in use: the card face, the four-state
reader light, the stamp re-press on change, the receipt rules and event stream, the soft-key row.
No unused device worth a fix in a fixed-size desktop tool.

## material_fixes

1. Setup surface: replace the near-miss palette constants with the OWN-WORLD values
   (#1D4E9E / #F4F1E8 / #1A1A1A / #2F9E63 / #C2402F) so both surfaces render one world (GROUND contradicted).
2. Setup surface: render the maintenance page inside the kiosk frame — card face left, receipt-white
   ground, soft keys — instead of unframed `ui.label`s on egui's default field (GROUND + FIRST VIEWPORT contradicted).

## keep

Keep the receipt-white ground and the reader-light discipline: green and vermilion stay status
lights, and no gradient or shadow enters any page.

Verdict pass (after fix commit `db49019`; recapture of `setup-maintenance.png`):

## verdict

- Palette unification — **resolved**: the recapture samples 49,111 px of #1D4E9E and 457,403 px of
  #F4F1E8 on the maintenance page; the former egui-gray field is gone (1 px of #F8F8F8 remains).
- Maintenance page kiosk frame — **resolved**: the recapture shows the card-blue card face with
  "GDUT Net 上网卡 / 学号 / 已装卡" on the left, the receipt-white field with the page content and
  the soft-key row below.
- Regressions introduced by the fix batch: none observed; the GUI surface files were not touched,
  and its captures still stand.

## remaining

clear (scored fixes only; the installer's Welcome/Account/Progress/Done pages remain uncaptured —
reaching them needs an interactive click-through on the elevated window, which the automation could
not perform reliably; they are verified by per-task code review instead).

`disposition: ship` (covers the two scored fixes, not the whole surface)
