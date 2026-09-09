<script>
  /**
   * Empty state. `md` is the Home screen's full-width version; `sm` is the
   * one-liner a panel shows when its list has nothing in it.
   *
   * @type {{
   *   title: string,
   *   body?: string,
   *   size?: 'md' | 'sm',
   *   children?: import('svelte').Snippet,
   * }}
   */
  let { title, body, size = 'md', children } = $props()
</script>

<div class="empty {size}">
  <div class="title">{title}</div>
  {#if body}<div class="body">{body}</div>{/if}
  {#if children}<div class="actions">{@render children()}</div>{/if}
</div>

<style>
  .empty { color: var(--t3); line-height: 1.7 }

  .md { padding: 52px 0; font-size: 12px }
  .sm { padding: 14px var(--s-2); font-size: 11px; line-height: 1.5 }

  .body { margin-top: var(--s-2) }
  .sm .body { margin-top: var(--s-1) }

  .actions { display: flex; flex-wrap: wrap; gap: var(--s-2); margin-top: var(--s-5) }
  .sm .actions { margin-top: var(--s-3) }
</style>
