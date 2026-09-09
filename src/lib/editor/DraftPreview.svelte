<script>
  import { draft } from './draft.svelte.js'
  import { editor } from '../state/editor.svelte.js'
  import { AI_STROKE_PX, paintedStroke, polylinePoints } from './gesture.js'
  import { t } from '../i18n/index.js'

  /**
   * What a gesture looks like while it is still a gesture: the mask it is
   * about to make, the path that described it, and - for Clone / heal - where
   * the stamp is reading from.
   *
   * **A stroke previews as the stroke.** It used to preview as its bounding
   * box, on the reasoning that a region *is* a bbox and showing an outline the
   * commit would not produce would be the lie. That reasoning was sound and its
   * premise was the bug: the commit produced a rectangle because the seam only
   * carried one. Now that the painted shape
   * crosses the seam, the honest preview of a brush stroke is the swept disc.
   * A Shapes gesture previews as the shape it actually sends: the rectangle
   * for a rectangle drag, the ellipse for an ellipse, and the closed outline
   * for a lasso or a polygon - which used to be drawn as the box around them
   * and no longer commits as one (`drawing.svelte.js#paintedShape`).
   *
   * The SVG's user units *are* page percent, which is the coordinate space
   * `gesture.js` works in, so nothing here converts anything - except the
   * stroke, whose group is scaled into **page pixels** so that a round cap is
   * round: a brush is circular in the page's own pixels, and the sheet's
   * non-square aspect turns a percent-space circle into an ellipse on screen.
   * Hairlines carry `vector-effect="non-scaling-stroke"` so they stay hairlines
   * at every zoom; the swept stroke deliberately does not, because its width is
   * the thing being shown.
   */

  /** @type {{pageId: string, pageWidth?: number, pageHeight?: number}} */
  let { pageId, pageWidth = 1600, pageHeight = 2400 } = $props()

  const active = $derived(draft.active?.pageId === pageId ? draft.active : null)
  const bbox = $derived(active?.bbox ?? null)
  const source = $derived(draft.cloneSource?.pageId === pageId ? draft.cloneSource : null)
  /**
   * Paint and clone / heal preview as **pixels** now (`PaintLayer.svelte`), so
   * the tinted capsule that stood in for them would only sit on top of the real
   * thing and mute it. The dashed cursor ring is `DrawLayer`'s and stays: it
   * says where the next stamp lands, which the painted pixels behind it cannot.
   */
  const livePixels = $derived(active?.tool === 'cloneHeal' || active?.tool === 'brush')

  /**
   * The **path the pointer took**, drawn as a hairline over the shape it
   * describes. It belongs to the two shapes that are being *built* out of
   * clicks - a lasso and a polygon - where the vertices so far are the only
   * sign of progress.
   *
   * It is deliberately not drawn over a `stroke`. The swept capsule already
   * says exactly what the stroke covers, and a 1px `--page-mark` line down the
   * middle of it read as a nib mark trailing the pointer - a second, sharper
   * shape at the same place as the real one, describing nothing the capsule
   * did not already describe.
   */
  const path = $derived(
    active &&
    active.points.length > 1 &&
    (active.kind === 'lasso' || active.kind === 'polygon') &&
    !livePixels
      ? polylinePoints(active.points)
      : '',
  )

  /**
   * The swept stroke, in page pixels: the same path and the same radius
   * `drawing.svelte.js#strokeOf` sends across the seam, read from the same
   * parameters, so what is drawn here is what will be cleaned.
   *
   * **Only the AI mask brush reaches this now.** It is the one stroke tool
   * whose commit is a *region* rather than pixels, so a tinted capsule is the
   * honest preview of it. The Brush and Clone / heal are `livePixels` and draw
   * their real colours in `PaintLayer.svelte`; the colour and opacity this
   * block used to paint the capsule with were left over from the Brush's mask
   * modes and could not be reached once the Brush became paint-only.
   */
  const swept = $derived.by(() => {
    if (active?.kind !== 'stroke' || livePixels) return null
    const params = editor.toolParams[active.tool] ?? {}
    const size = Number(params.size ?? (active.tool === 'aiMaskBrush' ? AI_STROKE_PX : 0))
    const stroke = paintedStroke(active.points, size)
    if (!stroke) return null
    return {
      width: stroke.radius * 2,
      points: stroke.points
        .map((point) => `${(point.x / 100) * pageWidth},${(point.y / 100) * pageHeight}`)
        .join(' '),
      // A one-point stroke has no polyline to draw, so it is a dot.
      dot: stroke.points.length < 2 ? stroke.points[0] : null,
    }
  })

  /**
   * A closed outline, for the two shapes that are one: a lasso is a freehand
   * polygon and a clicked polygon is the same thing with fewer samples. Both
   * used to preview as the **rectangle around them** with the path drawn on
   * top, which is exactly what the commit no longer does
   * (`drawing.svelte.js#paintedShape`) - so the preview was showing the old
   * defect while the seam had stopped producing it.
   */
  const closed = $derived(
    (active?.kind === 'lasso' || active?.kind === 'polygon') && (active?.points.length ?? 0) >= 3
      ? polylinePoints(active.points)
      : '',
  )

  /**
   * A Shapes gesture set to a solid colour previews **in that colour**: it is
   * paint rather than a clean, and the blue mark tint would say "an engine will
   * decide what goes here", which is the one thing it will not do.
   */
  const solidFill = $derived(
    active?.tool === 'shapes' && editor.toolParams.shapes?.mode === 'solid'
      ? {
          color: String(editor.toolParams.shapes?.color ?? '#000000'),
          opacity:
            Math.max(0, Math.min(100, Number(editor.toolParams.shapes?.opacity ?? 100))) / 100,
        }
      : null,
  )
  const shapeStyle = $derived(
    solidFill ? `fill: ${solidFill.color}; opacity: ${solidFill.opacity};` : undefined,
  )
</script>

<svg class="preview" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
  {#if source}
    <g class="source">
      <circle cx={source.x} cy={source.y} r="1.2" vector-effect="non-scaling-stroke" />
      <line x1={source.x - 2.4} y1={source.y} x2={source.x + 2.4} y2={source.y}
            vector-effect="non-scaling-stroke" />
      <line x1={source.x} y1={source.y - 2.4} x2={source.x} y2={source.y + 2.4}
            vector-effect="non-scaling-stroke" />
    </g>
  {/if}

  {#if swept}
    <g transform="scale({100 / pageWidth} {100 / pageHeight})">
      {#if swept.dot}
        <circle
          class="shape"
          cx={(swept.dot.x / 100) * pageWidth}
          cy={(swept.dot.y / 100) * pageHeight}
          r={swept.width / 2}
        />
      {:else}
        <!-- The width is a `style` and not an attribute on purpose: the
             stylesheet's `.shape { stroke-width }` would win over a
             presentation attribute and draw every stroke as a hairline. -->
        <polyline
          class="shape swept"
          points={swept.points}
          style="stroke-width: {swept.width}"
          stroke-linecap="round"
          stroke-linejoin="round"
        />
      {/if}
    </g>
  {:else if bbox && !livePixels}
    {#if active?.kind === 'ellipse'}
      <ellipse
        class="shape"
        cx={bbox.x + bbox.w / 2}
        cy={bbox.y + bbox.h / 2}
        rx={bbox.w / 2}
        ry={bbox.h / 2}
        vector-effect="non-scaling-stroke"
        style={shapeStyle}
      />
    {:else if closed}
      <polygon class="shape" points={closed} vector-effect="non-scaling-stroke" style={shapeStyle} />
    {:else if active?.kind !== 'lasso' && active?.kind !== 'polygon'}
      <rect
        class="shape"
        x={bbox.x}
        y={bbox.y}
        width={bbox.w}
        height={bbox.h}
        vector-effect="non-scaling-stroke"
        style={shapeStyle}
      />
    {/if}
  {/if}

  {#if path}
    <polyline
      class="path"
      class:closing={active?.kind === 'polygon'}
      points={path}
      vector-effect="non-scaling-stroke"
    />
  {/if}
</svg>

<!-- The gesture's size in words as well as in pixels, and the one number on
     the sheet that changes while a gesture runs. Polite rather than assertive:
     it should not interrupt, and a drag produces a great many of them. -->
{#if bbox}
  <p class="readout" aria-live="polite">
    {t('canvas.draft.size', { w: Math.round(bbox.w), h: Math.round(bbox.h) })}
  </p>
{/if}

<style>
  .preview {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
    overflow: visible;
  }

  /* Blue, not ink: the artwork under the preview is black ink, and a dark
     draft over a dark panel is a draft the user cannot see. */
  .shape {
    fill: var(--page-mark-tint);
    stroke: var(--page-mark-line);
    stroke-width: 1.5;
  }

  /* A swept stroke has no outline to draw: the stroke *is* the area, so the
     tint moves from the fill to the stroke and the width comes from the
     brush rather than from here. */
  .shape.swept {
    fill: none;
    stroke: var(--page-mark-tint);
  }

  .path {
    fill: none;
    stroke: var(--page-mark);
    stroke-width: 1;
    stroke-linejoin: round;
    stroke-linecap: round;
    opacity: .7;
  }

  /* A polygon that has not been closed yet says so by being dashed. */
  .path.closing { stroke-dasharray: 3 2 }

  .source {
    fill: none;
    stroke: var(--page-mark);
    stroke-width: 1.25;
  }

  .readout {
    position: absolute;
    right: 6px;
    bottom: 5px;
    margin: 0;
    padding: 2px 6px;
    border-radius: var(--r-xs);
    background: var(--paper);
    border: 1px solid var(--page-line);
    color: var(--page-ink);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: max(8px, 2.15cqw);
    line-height: 1.5;
    pointer-events: none;
    animation: mcFade var(--dur-fast) var(--ease);
  }
</style>
