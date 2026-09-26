<script>
  import { Modal, Button } from '../ui/index.js'
  import { getBackend } from '../api/backend.js'
  import { closeModal } from '../state/app.svelte.js'
  import { refreshBilling } from '../state/billing.svelte.js'
  import { t } from '../i18n/index.js'

  let { spec } = $props()
  let tokenId = $state('')
  let secret = $state('')
  let remember = $state(false)
  let busy = $state(false)
  let error = $state(false)
  const uid = $props.id()
  async function connect() {
    if (busy || !tokenId.trim() || !secret.trim()) return
    busy = true
    error = false
    try {
      await getBackend().storeCloudSecret({ provider: 'modal', profileId: spec.props.profileId, role: 'setup', tokenId: tokenId.trim(), secret: secret.trim(), sessionOnly: !remember })
      tokenId = ''; secret = ''
      if (!await refreshBilling(spec.props.profileId)) { error = true; return }
      closeModal('connected')
    } catch { error = true; secret = '' }
    finally { busy = false }
  }
</script>

<Modal title={t('settings.cloud.usage.connectTitle')} width={420} onclose={busy ? undefined : () => closeModal(null)}>
  <p>{t('settings.cloud.usage.connectHelp')}</p>
  <form onsubmit={(event) => { event.preventDefault(); void connect() }}>
    <label for={`${uid}-id`}>{t('settings.cloud.usage.tokenId')}</label>
    <input id={`${uid}-id`} bind:value={tokenId} autocomplete="off" spellcheck="false" disabled={busy} />
    <label for={`${uid}-secret`}>{t('settings.cloud.usage.tokenSecret')}</label>
    <input id={`${uid}-secret`} type="password" bind:value={secret} autocomplete="off" disabled={busy} />
    <label class="remember"><input type="checkbox" bind:checked={remember} disabled={busy} />{t('settings.cloud.usage.remember')}</label>
    {#if error}<p role="alert">{t('settings.cloud.usage.connectFailed')}</p>{/if}
    <Button type="submit" disabled={busy || !tokenId.trim() || !secret.trim()}>{t(busy ? 'settings.cloud.usage.connecting' : 'settings.cloud.usage.connect')}</Button>
  </form>
</Modal>

<style>
  p { color: var(--t2); font-size: 12px; line-height: 1.6; }
  form { display: grid; gap: 8px; }
  label { font-size: 12px; color: var(--t2); }
  input:not([type=checkbox]) { width: 100%; box-sizing: border-box; padding: 8px; color: var(--text); background: var(--surface); border: 1px solid var(--line); border-radius: var(--r-sm); }
  .remember { display: flex; align-items: center; gap: 6px; margin: 5px 0; }
</style>
