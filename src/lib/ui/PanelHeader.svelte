<script>
  import Icon from '../icons/Icon.svelte'

  /**
   * The 29px header that tops each editor panel: a tracked title, an optional
   * right-aligned count, and optional trailing controls.
   *
   * @type {{
   *   title: string,
   *   meta?: string,
   *   icon?: string,
   *   as?: string,
   *   children?: import('svelte').Snippet,
   * }}
   */
  let { title, meta, icon, as = 'h2', children } = $props()
</script>

<div class="panel-header">
  {#if icon}
    <span class="icon"><Icon name={icon} size={14} /></span>
  {/if}
  <svelte:element this={as} class="title">{title}</svelte:element>
  {#if meta}<div class="meta">{meta}</div>{:else}<div class="spacer"></div>{/if}
  {@render children?.()}
</div>

<style>
  .panel-header {
    display: flex;
    align-items: center;
    flex: none;
    gap: 7px;
    height: 29px;
    padding: 0 5px 0 7px;
    border-bottom: 1px solid var(--line);
  }
  .icon { display: flex; flex: none; color: var(--t3) }

  .title {
    flex: 0 0 auto;
    margin: 0;
    font-size: 10px;
    letter-spacing: .1em;
    font-weight: 600;
    text-transform: uppercase;
    color: var(--t2);
    white-space: nowrap;
  }

  .meta {
    flex: 1 1 auto;
    min-width: 0;
    text-align: right;
    font-size: 10.5px;
    color: var(--t3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .spacer { flex: 1 1 auto }
</style>
