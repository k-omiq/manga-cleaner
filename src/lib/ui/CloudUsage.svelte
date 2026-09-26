<script>
  import { getBackend } from '../api/backend.js'
  import { session } from '../state/session.svelte.js'
  import { cloud } from '../state/cloud.svelte.js'
  import { pushModal } from '../state/app.svelte.js'
  import { billing, refreshBilling } from '../state/billing.svelte.js'
  import { t } from '../i18n/index.js'
  import { formatUsageCost, monthStartMs } from '../model/cloud-usage.js'

  let usage = $state(null)
  let failed = $state(false)
  let expanded = $state(false)
  let hasBillingSecret = $state(false)
  let disconnecting = $state(false)
  let disconnectFailed = $state(false)
  let connectionEpoch = 0
  $effect(() => {
    const backend = getBackend()
    let live = true
    let pending = false
    const refresh = async () => {
      if (pending) return
      pending = true
      try {
        const result = await backend.getCloudUsage({ monthStartMs: monthStartMs() })
        if (live) { usage = result; failed = false }
      } catch { if (live) failed = true }
      finally { pending = false }
    }
    void refresh()
    const timer = setInterval(refresh, 5000)
    return () => { live = false; clearInterval(timer) }
  })
  const modalProfile = $derived(cloud.readiness.target?.type === 'modal' ? cloud.readiness.target.profile_id : null)
  $effect(() => {
    const id = modalProfile
    hasBillingSecret = false
    if (!id) { void refreshBilling(null); return }
    let live = true
    const tick = async () => {
      const epoch = connectionEpoch
      const summary = await getBackend().getCloudSecretSummary({ provider: 'modal', profileId: id, role: 'setup' }).catch(() => null)
      if (!live || epoch !== connectionEpoch || disconnecting) return
      hasBillingSecret = summary?.present === true
      if (hasBillingSecret) await refreshBilling(id)
    }
    void tick()
    const timer = setInterval(tick, 60000)
    return () => { live = false; clearInterval(timer) }
  })
  async function disconnectBilling() {
    if (disconnecting || !modalProfile) return
    const id = modalProfile
    connectionEpoch += 1
    disconnecting = true
    disconnectFailed = false
    try {
      await getBackend().deleteCloudSecret({ provider: 'modal', profileId: id, role: 'setup' })
      if (modalProfile === id) {
        hasBillingSecret = false
        await refreshBilling(null)
      }
    } catch { disconnectFailed = true }
    finally { disconnecting = false }
  }
  const dollars = (value) => new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', maximumFractionDigits: 4 }).format(value)
  const visible = $derived(session.cloudAllowed || cloud.readiness.configured || usage?.month.attempts > 0 || usage?.session.attempts > 0 || usage?.unreadableAttempts > 0)
</script>

{#if visible}
  <section class="usage" aria-label={t('settings.cloud.usage.title')}>
    <button class="heading" aria-expanded={expanded} onclick={() => expanded = !expanded}>
      <span>{t('settings.cloud.usage.title')}</span><span aria-hidden="true">{expanded ? '−' : '+'}</span>
    </button>
    {#if failed}
      <p role="status">{t('settings.cloud.usage.unavailable')}</p>
    {:else if usage}
      <dl>
        <div><dt>{t('settings.cloud.usage.month')}</dt><dd>{formatUsageCost(usage.month, t('settings.cloud.usage.unreported'), usage.unreadableAttempts > 0)}</dd></div>
        <div><dt>{t('settings.cloud.usage.session')}</dt><dd>{formatUsageCost(usage.session, t('settings.cloud.usage.unreported'), usage.unreadableAttempts > 0)}</dd></div>
      </dl>
      {#if usage.unreadableAttempts}<p role="status">{t('settings.cloud.usage.incomplete')}</p>{/if}
      {#if expanded}
        {#if billing.data && billing.profileId === modalProfile}
          <p>{t('settings.cloud.usage.workspace', { name: billing.data.workspace })}</p>
          <dl>
            <div><dt>{t('settings.cloud.usage.workspaceMonth')}</dt><dd>{dollars(billing.data.metered_cost)}</dd></div>
            <div><dt>{t('settings.cloud.usage.billingChange')}</dt><dd>{dollars(billing.data.sessionChange)}</dd></div>
          </dl>
          <p>{t('settings.cloud.usage.billingScope')}</p>
        {:else if modalProfile}
          <button class="connect" onclick={() => pushModal({ kind: 'cloudBilling', props: { profileId: modalProfile } })}>{t('settings.cloud.usage.connect')}</button>
          {#if billing.failed}<p>{t('settings.cloud.usage.billingFailed')}</p>{/if}
        {/if}
        {#if modalProfile && (hasBillingSecret || billing.data && billing.profileId === modalProfile)}
          <button class="connect" disabled={disconnecting} onclick={disconnectBilling}>{t('settings.cloud.usage.disconnect')}</button>
          {#if disconnectFailed}<p role="alert">{t('settings.cloud.usage.disconnectFailed')}</p>{/if}
        {/if}
        <p>{t('settings.cloud.usage.requests', { month: usage.month.attempts, session: usage.session.attempts })}</p>
        {#if usage.month.unpricedAttempts || usage.session.unpricedAttempts}
          <p>{t('settings.cloud.usage.unpriced', { month: usage.month.unpricedAttempts, session: usage.session.unpricedAttempts })}</p>
        {/if}
        <p>{t('settings.cloud.usage.scope')}</p>
      {/if}
    {:else}
      <p>{t('settings.cloud.usage.loading')}</p>
    {/if}
  </section>
{/if}

<style>
  .usage { width: 230px; max-width: calc(100vw - 28px); padding: 8px 9px; border-radius: var(--r-md); background: var(--surface); box-shadow: var(--edge); }
  .heading { display: flex; justify-content: space-between; width: 100%; padding: 0; border: 0; background: transparent; color: var(--t3); font: inherit; font-size: 10px; font-weight: 600; letter-spacing: .04em; text-transform: uppercase; cursor: pointer; }
  dl { margin: 6px 0 0; font-size: 11px; color: var(--t2); }
  dl div { display: flex; justify-content: space-between; gap: 8px; line-height: 1.8; }
  dd { margin: 0; font-variant-numeric: tabular-nums; }
  .connect { margin-top: 8px; border: 0; padding: 0; background: transparent; color: var(--text); font: inherit; font-size: 11px; cursor: pointer; text-decoration: underline; }
  p { margin: 5px 0 0; color: var(--t3); font-size: 10px; line-height: 1.5; }
</style>
