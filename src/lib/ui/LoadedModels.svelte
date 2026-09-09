<script>
  import Icon from '../icons/Icon.svelte'

  /**
   * What is using memory right now, bottom-left, above the notice stack.
   *
   * **It draws nothing when nothing is loaded**, which is most of the time: an
   * application that is not cleaning anything holds no sessions, and a panel
   * sitting empty in the corner would be a permanent reminder of a thing the
   * reader does not otherwise have to think about. It appears when a run or an
   * edit loads something and goes when that is given back.
   *
   * A primitive: no backend, no i18n, no timers. Every string arrives already
   * translated, the same contract `Notice.svelte` keeps - which is what lets
   * the whole panel be rendered in a test with three rows and no Tauri window.
   *
   * @type {{
   *   models: Array<{ id: number, name: string, size: string, device: string, unloading: boolean, unloadLabel: string }>,
   *   title: string,
   *   hint: string,
   *   unloadingLabel: string,
   *   onunload: (id: number) => void,
   * }}
   */
  let { models, title, hint, unloadingLabel, onunload } = $props()
</script>

{#if models.length > 0}
  <section class="tab" aria-label={title}>
    <header class="head">
      <span class="mark"><Icon name="cpu" size={12} /></span>
      <h2 class="title">{title}</h2>
    </header>
    <ul class="rows">
      {#each models as model (model.id)}
        <li class="row">
          <span class="name" title={model.device}>{model.name}</span>
          {#if model.unloading}
            <!-- The request is in flight: the run drops the session at its
                 next region boundary, so the row stays and says so rather
                 than vanishing and claiming memory that is still held. -->
            <span class="state">{unloadingLabel}</span>
          {:else}
            <span class="size">{model.size}</span>
            <button
              type="button"
              class="close"
              onclick={() => onunload(model.id)}
              title={model.unloadLabel}
              aria-label={model.unloadLabel}
            >
              <Icon name="close" size={11} />
            </button>
          {/if}
        </li>
      {/each}
    </ul>
    <p class="hint">{hint}</p>
  </section>
{/if}

<style>
  .tab {
    width: 230px;
    max-width: calc(100vw - 28px);
    padding: 8px 9px 7px;
    border-radius: var(--r-md);
    background: var(--surface);
    box-shadow: var(--edge);
    animation: mcIn var(--dur) var(--ease);
  }

  .head {
    display: flex;
    gap: var(--s-3);
    align-items: center;
    margin-bottom: 5px;
  }
  .mark { display: flex; flex: none; color: var(--t3) }
  .title {
    margin: 0;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--t3);
  }

  .rows { margin: 0; padding: 0; list-style: none }

  .row {
    display: flex;
    gap: var(--s-3);
    align-items: center;
    min-height: 20px;
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    font-size: 11px;
    line-height: 1.45;
    color: var(--t2);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Tabular figures so a column of sizes does not jitter as it updates. */
  .size {
    flex: none;
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    color: var(--t3);
  }

  .state {
    flex: none;
    font-size: 11px;
    font-style: italic;
    color: var(--t3);
  }

  .close {
    display: flex;
    flex: none;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    margin-right: -2px;
    padding: 0;
    border: none;
    border-radius: var(--r-xs);
    background: transparent;
    color: var(--t3);
    cursor: pointer;
    transition: color var(--dur-fast) var(--ease);
  }
  .close:hover { color: var(--text) }

  .hint {
    margin: 4px 0 0;
    font-size: 10px;
    line-height: 1.4;
    color: var(--t3);
  }
</style>
