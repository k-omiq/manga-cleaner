<script>
  import { t } from '../i18n/index.js'

  /**
   * The provenance an expanded mask row shows: a 70px key column and the
   * value beside it.
   *
   * A value is either words - a rung name, a fill mode, an elapsed time - in
   * which case it arrives as a key and is translated, or it is an identifier -
   * a model version, a cloud request id - in which case it is printed as it
   * is. `maskrows.js` decides which; this only renders.
   *
   * @type {{ facts: import('./maskrows.js').Fact[] }}
   */
  let { facts } = $props()
</script>

<dl class="facts">
  {#each facts as fact (fact.key)}
    <div class="fact">
      <dt>{t(fact.key)}</dt>
      <dd>{fact.valueKey ? t(fact.valueKey, fact.params) : fact.value}</dd>
    </div>
  {/each}
</dl>

<style>
  .facts { margin: 0 }

  .fact {
    display: flex;
    gap: var(--s-3);
    font-size: 10.5px;
    line-height: 1.7;
  }

  dt {
    flex: none;
    width: 70px;
    color: var(--t3);
  }

  dd {
    flex: 1;
    min-width: 0;
    margin: 0;
    color: var(--t2);
    overflow-wrap: anywhere;
  }
</style>
