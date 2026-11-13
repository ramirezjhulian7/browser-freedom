# E5: subconjuntos de `css/css-flexbox` de los criterios 3.3, 3.4 y 3.6 del roadmap

Medido el 2026-11-12 en macOS sobre `army/e5` (ronda 5, `master` `cd8fc5a` más los cambios de
E5), con `REFTEST_GROUPS=css/css-flexbox,css/css-writing-modes cargo test -p bf-paint --test
wpt_reftests_groups -- --ignored --test-threads=3`. Solo reftests (los de testharness no
cuentan). Cada subconjunto son los reftests cuyo archivo o referencia (`rel=match`) contiene el
texto indicado; un test puede estar en dos. Para E3 (roadmap).

| Criterio | `master` `cd8fc5a` | `army/e5` | Meta |
|---|---:|---:|---|
| 3.3 `linear-gradient` | 15/16 | 15/16 | todos |
| 3.4 `writing-mode`/rtl | 95/136 (69,9 %) | **103/136 (75,7 %)** | ≥ 75 % |
| 3.6 palabras clave de tamaño | 74/108 | 74/108 | todos |
| `css/selectors/of-type-selectors.xhtml` | pasa | pasa | pasa |

## Causas de los fallos

3.3 (1): `align-self-016`: no es el gradiente. Un ítem `align-self: start` en un contenedor
`column wrap` de 100 px con dos floats de 100 px: los navegadores le dan su ancho max-content
(200 px) y aquí sale fit-content (100 px).

3.4 (33):
- Baseline en flujos verticales o sintetizada (11): `baseline-synthesis-001/003/004`,
  `baseline-synthesis-vert-lr-line-under`, `synthesize-vrl-baseline`,
  `flex-align-baseline-column-vert-{lr,rl}-rtl-wrap-reverse`,
  `flexbox-align-self-baseline-horiz-006/008`, `flex-container-line-if-empty-vertical-writing-mode`,
  `flexbox_align-items-stretch-3`. Un fragmento vertical pierde su baseline al volver a físico
  (`fragment_to_physical`).
- `sideways-lr` (5): `flexbox-writing-mode-slr`, `-slr-rtl`, `-slr-row-mix`,
  `justify-content-sideways-001` y `flexbox-writing-mode-015`. `sideways-lr` se trata como
  `vertical-lr` (pendiente de E9).
- `align-self` en vertical con rtl (5): `flexbox-align-self-vert-rtl-001…005`.
- `safe` respecto al containing block exterior (3, tentative):
  `flex-abspos-align-self-safe-outer-cb-001…003`.
- `flex-wrap: balance` (1): `balance/balance-available-size-002` (no implementado).
- Tablas como ítem en vertical (1): `table-as-item-specified-width-vertical`.
- Sueltos: `flexbox-writing-mode-011/012` (ítems de varios modos de escritura con márgenes e
  `inline-size` lógico; sin analizar en detalle),
  `aspect-ratio-intrinsic-size-009`, `flexbox-flex-wrap-flexing-003`,
  `flexbox-align-self-horiz-002`, `flexbox-basic-canvas-vert-001v` (antialias, 392 px),
  `flexbox-mbp-horiz-001-rtl` (49 px de texto).

3.6 (34):
- `flex-wrap: balance` (8): `balance/balance-002` y `balance-line-count-intrinsic-001…007`.
- Mínimo automático (`min-width`/`min-height: auto`, §4.5) (8):
  `flexbox-min-{width,height}-auto-001` (28–30 px, texto) y `-002a/b/c`.
- Tamaños intrínsecos de contenedores en columna con wrap (5):
  `intrinsic-size/col-wrap-012/015/016/017`, `row-wrap-002.tentative`.
- `gap` con palabras clave o porcentajes (3): `gap-015`, `gap-collapse`, `gap-percentage-wrap`.
- Tablas como ítem (2): `table-as-flex-item-max-content`, `table-as-item-fixed-min-width-2`.
- Contenedor con `min-content`/`max-content` (2): `flex-container-max-content-001`,
  `flex-container-min-content-001`.
- Sueltos: `align-baseline`, `align-self-016` (ver 3.3), `flex-item-max-width-min-content-002`,
  `flex-minimum-height-flex-items-023`, `flex-minimum-width-flex-items-013`,
  `percentage-heights-022`.

# Lista completa

## 3.3 — `linear-gradient` en el test o su referencia: 15/16

- pasa `align-content-001.htm`
- pasa `align-content-002.htm`
- pasa `align-content-003.htm`
- pasa `align-content-004.htm`
- pasa `align-content-005.htm`
- pasa `align-content-007.htm`
- pasa `align-items-001.htm`
- pasa `align-items-002.htm`
- pasa `align-items-003.htm`
- pasa `align-items-004.htm`
- **falla** `align-self-016.html`
- pasa `justify-content-001.htm`
- pasa `justify-content-002.htm`
- pasa `justify-content-003.htm`
- pasa `justify-content-004.htm`
- pasa `justify-content-005.htm`

## 3.4 — `writing-mode` o rtl en el test o su referencia: 103/136

- pasa `abspos/abspos-autopos-htb-ltr.html`
- pasa `abspos/abspos-autopos-htb-rtl.html`
- pasa `abspos/abspos-autopos-vlr-ltr.html`
- pasa `abspos/abspos-autopos-vlr-rtl.html`
- pasa `abspos/abspos-autopos-vrl-ltr.html`
- pasa `abspos/abspos-autopos-vrl-rtl.html`
- **falla** `abspos/flex-abspos-align-self-safe-outer-cb-001.tentative.html`
- **falla** `abspos/flex-abspos-align-self-safe-outer-cb-002.tentative.html`
- **falla** `abspos/flex-abspos-align-self-safe-outer-cb-003.tentative.html`
- pasa `abspos/flex-abspos-staticpos-align-self-safe-001.html`
- pasa `abspos/flex-abspos-staticpos-align-self-safe-002.html`
- pasa `abspos/flex-abspos-staticpos-align-self-safe-003.html`
- pasa `alignment/flex-align-baseline-column-rtl-direction.html`
- **falla** `alignment/flex-align-baseline-column-vert-lr-rtl-wrap-reverse.html`
- **falla** `alignment/flex-align-baseline-column-vert-rl-rtl-wrap-reverse.html`
- **falla** `aspect-ratio-intrinsic-size-009.html`
- pasa `auto-margins-003.html`
- **falla** `balance/balance-available-size-002.html`
- **falla** `baseline-synthesis-001.html`
- pasa `baseline-synthesis-002.html`
- **falla** `baseline-synthesis-003.html`
- **falla** `baseline-synthesis-004.html`
- **falla** `baseline-synthesis-vert-lr-line-under.html`
- pasa `cross-axis-scrollbar.html`
- pasa `css-flexbox-row-reverse-wrap-reverse.html`
- pasa `css-flexbox-row-reverse-wrap.html`
- pasa `css-flexbox-row-reverse.html`
- pasa `css-flexbox-row-wrap-reverse.html`
- pasa `css-flexbox-row-wrap.html`
- pasa `css-flexbox-row.html`
- pasa `css-flexbox-test1.html`
- pasa `fit-content-item-002.html`
- pasa `fit-content-item-003.html`
- pasa `fit-content-item-004.html`
- pasa `flex-aspect-ratio-img-column-008.html`
- pasa `flex-aspect-ratio-img-vert-lr.html`
- **falla** `flex-container-line-if-empty-vertical-writing-mode.html`
- pasa `flex-direction-row-vertical.html`
- pasa `flex-item-max-width-min-content.html`
- pasa `flex-item-min-width-min-content.html`
- **falla** `flexbox-align-self-baseline-horiz-006.xhtml`
- **falla** `flexbox-align-self-baseline-horiz-008.xhtml`
- **falla** `flexbox-align-self-horiz-002.xhtml`
- pasa `flexbox-align-self-vert-002.xhtml`
- **falla** `flexbox-align-self-vert-rtl-001.xhtml`
- **falla** `flexbox-align-self-vert-rtl-002.xhtml`
- **falla** `flexbox-align-self-vert-rtl-003.xhtml`
- **falla** `flexbox-align-self-vert-rtl-004.xhtml`
- **falla** `flexbox-align-self-vert-rtl-005.xhtml`
- pasa `flexbox-basic-block-horiz-001v.xhtml`
- pasa `flexbox-basic-block-vert-001v.xhtml`
- pasa `flexbox-basic-canvas-horiz-001v.xhtml`
- **falla** `flexbox-basic-canvas-vert-001v.xhtml`
- pasa `flexbox-column-row-gap-002.html`
- pasa `flexbox-column-row-gap-004.html`
- pasa `flexbox-flex-direction-column-reverse.htm`
- pasa `flexbox-flex-direction-column.htm`
- pasa `flexbox-flex-direction-default.htm`
- pasa `flexbox-flex-direction-row-reverse.htm`
- pasa `flexbox-flex-direction-row.htm`
- **falla** `flexbox-flex-wrap-flexing-003.html`
- pasa `flexbox-flex-wrap-wrap-reverse.htm`
- pasa `flexbox-flex-wrap-wrap.htm`
- pasa `flexbox-justify-content-horiz-006.xhtml`
- pasa `flexbox-justify-content-wmvert-001.xhtml`
- pasa `flexbox-mbp-horiz-001-rtl-reverse.xhtml`
- **falla** `flexbox-mbp-horiz-001-rtl.xhtml`
- pasa `flexbox-mbp-horiz-002v.xhtml`
- pasa `flexbox-mbp-horiz-003v.xhtml`
- pasa `flexbox-overflow-padding-002.html`
- pasa `flexbox-safe-overflow-position-005.html`
- pasa `flexbox-vert-lr-with-img.html`
- pasa `flexbox-writing-mode-001.html`
- pasa `flexbox-writing-mode-002.html`
- pasa `flexbox-writing-mode-003.html`
- pasa `flexbox-writing-mode-004.html`
- pasa `flexbox-writing-mode-005.html`
- pasa `flexbox-writing-mode-006.html`
- pasa `flexbox-writing-mode-007.html`
- pasa `flexbox-writing-mode-008.html`
- pasa `flexbox-writing-mode-009.html`
- pasa `flexbox-writing-mode-010.html`
- **falla** `flexbox-writing-mode-011.html`
- **falla** `flexbox-writing-mode-012.html`
- pasa `flexbox-writing-mode-013.html`
- pasa `flexbox-writing-mode-014.html`
- **falla** `flexbox-writing-mode-015.html`
- pasa `flexbox-writing-mode-016.html`
- **falla** `flexbox-writing-mode-slr-row-mix.html`
- **falla** `flexbox-writing-mode-slr-rtl.html`
- **falla** `flexbox-writing-mode-slr.html`
- pasa `flexbox-writing-mode-srl-row-mix.html`
- pasa `flexbox-writing-mode-srl-rtl.html`
- pasa `flexbox-writing-mode-srl.html`
- pasa `flexbox_align-items-center-3.html`
- **falla** `flexbox_align-items-stretch-3.html`
- pasa `flexbox_justifycontent-end-rtl.html`
- pasa `flexbox_justifycontent-start-rtl.html`
- pasa `flexbox_rtl-direction.html`
- pasa `flexbox_rtl-flow-reverse.html`
- pasa `flexbox_rtl-flow.html`
- pasa `flexbox_rtl-order.html`
- pasa `flexbox_writing_mode_vertical_lays_out_contents_from_top_to_bottom.html`
- pasa `gap-001-lr.html`
- pasa `gap-001-rl.html`
- pasa `gap-001-rtl.html`
- pasa `gap-002-lr.html`
- pasa `gap-002-rl.html`
- pasa `gap-002-rtl.html`
- pasa `gap-003-lr.html`
- pasa `gap-003-rl.html`
- pasa `gap-003-rtl.html`
- pasa `gap-004-lr.html`
- pasa `gap-004-rl.html`
- pasa `gap-004-rtl.html`
- pasa `gap-005-lr.html`
- pasa `gap-005-rl.html`
- pasa `gap-005-rtl.html`
- pasa `gap-006-lr.html`
- pasa `gap-006-rl.html`
- pasa `gap-006-rtl.html`
- pasa `gap-007-lr.html`
- pasa `gap-007-rl.html`
- pasa `gap-007-rtl.html`
- pasa `grid-flex-item-001.html`
- pasa `grid-flex-item-004.html`
- **falla** `justify-content-sideways-001.html`
- pasa `ortho-table-item-001.html`
- pasa `overflow-area-002.html`
- pasa `overflow-top-left.html`
- pasa `percentage-size-subitems-001.html`
- pasa `stretch-obeys-min-max-002.html`
- pasa `stretching-orthogonal-flows.html`
- **falla** `synthesize-vrl-baseline.html`
- **falla** `table-as-item-specified-width-vertical.html`
- pasa `webkit-box-vertical-writing-mode.html`

## 3.6 — `min-content`, `max-content` o `fit-content` en el test o su referencia: 74/108

- **falla** `align-baseline.html`
- pasa `align-content-wrap-004.html`
- pasa `align-items-006.html`
- pasa `align-items-007.html`
- **falla** `align-self-016.html`
- **falla** `balance/balance-002.html`
- **falla** `balance/balance-line-count-intrinsic-001.html`
- **falla** `balance/balance-line-count-intrinsic-002.html`
- **falla** `balance/balance-line-count-intrinsic-003.html`
- **falla** `balance/balance-line-count-intrinsic-004.html`
- **falla** `balance/balance-line-count-intrinsic-005.html`
- **falla** `balance/balance-line-count-intrinsic-006.html`
- **falla** `balance/balance-line-count-intrinsic-007.html`
- pasa `column-flex-child-center-with-indefinite-height.html`
- pasa `fit-content-item-001.html`
- pasa `fit-content-item-002.html`
- pasa `fit-content-item-003.html`
- pasa `fit-content-item-004.html`
- **falla** `flex-container-max-content-001.html`
- **falla** `flex-container-min-content-001.html`
- pasa `flex-height-min-content.html`
- pasa `flex-item-content-is-min-width-max-content.html`
- pasa `flex-item-max-height-min-content.html`
- **falla** `flex-item-max-width-min-content-002.html`
- pasa `flex-item-max-width-min-content.html`
- pasa `flex-item-min-height-min-content-overflow.html`
- pasa `flex-item-min-height-min-content.html`
- pasa `flex-item-min-width-min-content-max-width.html`
- pasa `flex-item-min-width-min-content.html`
- pasa `flex-item-percentage-in-nested-flex-bsize-001.html`
- pasa `flex-item-percentage-in-nested-flex-bsize-002.html`
- pasa `flex-minimum-height-flex-items-001.xht`
- pasa `flex-minimum-height-flex-items-002.xht`
- pasa `flex-minimum-height-flex-items-003.xht`
- pasa `flex-minimum-height-flex-items-004.xht`
- pasa `flex-minimum-height-flex-items-006.xht`
- pasa `flex-minimum-height-flex-items-007.xht`
- pasa `flex-minimum-height-flex-items-008.xht`
- pasa `flex-minimum-height-flex-items-011.xht`
- pasa `flex-minimum-height-flex-items-017.html`
- **falla** `flex-minimum-height-flex-items-023.html`
- pasa `flex-minimum-height-flex-items-024.html`
- pasa `flex-minimum-width-flex-items-001.xht`
- pasa `flex-minimum-width-flex-items-002.xht`
- pasa `flex-minimum-width-flex-items-003.xht`
- pasa `flex-minimum-width-flex-items-004.xht`
- pasa `flex-minimum-width-flex-items-006.xht`
- pasa `flex-minimum-width-flex-items-007.xht`
- pasa `flex-minimum-width-flex-items-008.xht`
- **falla** `flex-minimum-width-flex-items-013.html`
- pasa `flex-wrap-002.html`
- pasa `flex-wrap-003.html`
- pasa `flex-wrap-004.html`
- pasa `flex-wrap-005.html`
- pasa `flexbox-flex-basis-content-003a.html`
- pasa `flexbox-flex-basis-content-003b.html`
- pasa `flexbox-flex-basis-content-004a.html`
- pasa `flexbox-flex-basis-content-004b.html`
- **falla** `flexbox-min-height-auto-001.html`
- **falla** `flexbox-min-height-auto-002a.html`
- **falla** `flexbox-min-height-auto-002b.html`
- **falla** `flexbox-min-height-auto-002c.html`
- **falla** `flexbox-min-width-auto-001.html`
- **falla** `flexbox-min-width-auto-002a.html`
- **falla** `flexbox-min-width-auto-002b.html`
- **falla** `flexbox-min-width-auto-002c.html`
- **falla** `gap-015.html`
- pasa `gap-016.html`
- pasa `gap-019.html`
- pasa `gap-020.html`
- pasa `gap-021.html`
- **falla** `gap-collapse.html`
- **falla** `gap-percentage-wrap.html`
- pasa `image-nested-within-definite-column-flexbox.html`
- pasa `inline-flex-min-content-height.html`
- pasa `intrinsic-size/col-wrap-001.html`
- pasa `intrinsic-size/col-wrap-002.html`
- pasa `intrinsic-size/col-wrap-003.html`
- pasa `intrinsic-size/col-wrap-006.html`
- pasa `intrinsic-size/col-wrap-007.html`
- pasa `intrinsic-size/col-wrap-008.html`
- pasa `intrinsic-size/col-wrap-011.html`
- **falla** `intrinsic-size/col-wrap-012.html`
- pasa `intrinsic-size/col-wrap-013.html`
- pasa `intrinsic-size/col-wrap-014.html`
- **falla** `intrinsic-size/col-wrap-015.html`
- **falla** `intrinsic-size/col-wrap-016.html`
- **falla** `intrinsic-size/col-wrap-017.html`
- pasa `intrinsic-size/col-wrap-020.html`
- pasa `intrinsic-size/row-001.html`
- pasa `intrinsic-size/row-002.html`
- pasa `intrinsic-size/row-003.html`
- pasa `intrinsic-size/row-004.html`
- pasa `intrinsic-size/row-006.html`
- pasa `intrinsic-size/row-007.html`
- pasa `intrinsic-size/row-009.html`
- pasa `intrinsic-size/row-010.html`
- **falla** `intrinsic-size/row-wrap-002.tentative.html`
- **falla** `percentage-heights-022.html`
- pasa `percentage-heights-023.html`
- **falla** `table-as-flex-item-max-content.html`
- **falla** `table-as-item-fixed-min-width-2.html`
- pasa `table-as-item-fixed-min-width-3.html`
- pasa `table-as-item-fixed-min-width.html`
- pasa `table-as-item-min-content-height-1.tentative.html`
- pasa `table-as-item-min-content-height-2.tentative.html`
- pasa `table-as-item-percent-width-cell-001.html`
- pasa `table-as-item-stretch-cross-size-5.html`
