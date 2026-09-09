<script>
  import { regionMenuSections } from '../model/masks.js'
  import { capabilities } from '../state/capabilities.svelte.js'
  import { runRegionMenuItem } from './maskactions.svelte.js'
  import { ContextMenu } from '../ui/index.js'
  import { t } from '../i18n/index.js'

  /**
   * The region context menu - Try again, Clean with…, Delete - wherever a
   * region can be right-clicked: on the canvas (`RegionLayer`), on the canvas
   * under a drawing tool's surface (`DrawLayer`), and on a Layers row
   * (`MaskRow`).
   *
   * One component for the three of them, so a menu raised over the page and a
   * menu raised over the row it belongs to cannot end up offering different
   * things. What it offers is `model/masks.js#regionMenuSections`, which is
   * pure and tested; what an entry *does* is
   * `maskactions.svelte.js#runRegionMenuItem`, which is the row's own controls
   * and no second implementation of them.
   *
   * Controlled: the host owns `at` - `{x, y, region}` in client pixels, or
   * `null` - and closes the menu by clearing it.
   *
   * @type {{
   *   at: {x: number, y: number, region: import('../api/backend.js').ApiRegion} | null,
   *   onclose: () => void,
   * }}
   */
  let { at, onclose } = $props()

  const sections = $derived(
    at
      ? regionMenuSections(at.region, { engines: capabilities.engines }).map((section) => ({
          id: section.id,
          label: section.labelKey ? t(section.labelKey) : null,
          items: section.items.map((item) => ({
            id: item.id,
            label: t(item.labelKey),
            icon: item.icon,
            selected: item.selected,
          })),
        }))
      : [],
  )

  /** @param {string} id */
  function pick(id) {
    // Read the region off the open menu before the host clears it.
    const region = at?.region
    if (region) runRegionMenuItem(id, region)
  }
</script>

{#if at}
  <ContextMenu
    x={at.x}
    y={at.y}
    label={t('masks.menu.label')}
    {sections}
    onselect={pick}
    {onclose}
  />
{/if}
