<script>
  import { glyph, icons } from './paths.js'

  /**
   * @type {{ name: string, size?: number, strokeWidth?: number }}
   */
  let { name, size = 16, strokeWidth = 1.5 } = $props()

  // Normalisation lives in paths.js and has exactly one implementation. All
  // this component adds is the dev-only guard; in production an unknown name
  // renders nothing rather than taking the app down.
  const g = $derived.by(() => {
    if (!(name in icons) && import.meta.env.DEV) {
      throw new Error(`Icon: unknown name "${name}"`)
    }
    return glyph(name)
  })
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 16 16"
  fill="none"
  stroke="currentColor"
  stroke-width={strokeWidth}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  focusable="false"
>
  {#each g.paths as d (d)}
    <path {d} />
  {/each}
  {#each g.filled as d (d)}
    <path {d} fill="currentColor" stroke="none" />
  {/each}
</svg>

<style>
  svg { display: block; flex: none }
</style>
