<script module>
  /**
   * Validate that a profile identifier is bounded, non-empty, and conforms to
   * the backend format rules (1..=64 chars, ASCII alphanumeric, '-', '_',
   * starting with alphanumeric).
   *
   * @param {string} id
   * @returns {boolean}
   */
  export function isValidProfileId(id) {
    if (!id || typeof id !== 'string') return false
    if (id !== id.trim()) return false
    if (id.length === 0 || id.length > 64) return false
    return /^[a-zA-Z0-9][a-zA-Z0-9_-]*$/.test(id)
  }

  /**
   * Validate human-readable display name (1..=128 chars, non-empty, trimmed, no control chars).
   *
   * @param {string} name
   * @returns {boolean}
   */
  export function isValidProfileName(name) {
    if (!name || typeof name !== 'string') return false
    if (name !== name.trim()) return false
    if (name.length === 0 || name.length > 128) return false
    if (/[\x00-\x1F\x7F]/.test(name)) return false
    return true
  }

  /**
   * Validate that an endpoint URL uses strict HTTPS, contains no credentials,
   * query strings, fragments, control characters, or prohibited hostnames (IPs, localhost).
   *
   * @param {string} rawUrl
   * @returns {boolean}
   */
  export function isValidEndpointUrl(rawUrl) {
    if (!rawUrl || typeof rawUrl !== 'string') return false
    if (rawUrl !== rawUrl.trim()) return false
    if (rawUrl.length === 0 || rawUrl.length > 2048) return false
    if (/[\x00-\x1F\x7F]/.test(rawUrl)) return false

    try {
      const parsed = new URL(rawUrl)
      if (parsed.protocol !== 'https:') return false
      if (parsed.username || parsed.password) return false
      if (parsed.search || parsed.hash) return false

      const host = parsed.hostname.toLowerCase().replace(/\.+$/, '')
      if (!host) return false
      if (
        host === 'localhost' ||
        host.endsWith('.localhost') ||
        host.endsWith('.local') ||
        host.endsWith('.internal') ||
        host.endsWith('.arpa')
      ) {
        return false
      }
      // Conservative rejection of IPv4 literals
      if (/^(\d{1,3}\.){3}\d{1,3}$/.test(host)) return false
      // Conservative rejection of IPv6 literals
      if (host.startsWith('[') || host.includes(':')) return false
      // WHATWG decimal notation
      if (/^\d+$/.test(host)) return false
      // Valid hostname characters
      if (!/^[a-z0-9_.-]+$/.test(host)) return false

      return true
    } catch {
      return false
    }
  }

  /**
   * Compare two endpoint URLs to check if their canonical origins differ.
   *
   * @param {string} urlA
   * @param {string} urlB
   * @returns {boolean}
   */
  export function isOriginChanged(urlA, urlB) {
    if (!urlA || !urlB) return false
    try {
      const originA = new URL(urlA).origin.toLowerCase()
      const originB = new URL(urlB).origin.toLowerCase()
      return originA !== originB
    } catch {
      return urlA !== urlB
    }
  }
</script>

<script>
  /**
   * Inference Settings Tab (P3c2)
   *
   * Manages public cloud inference profiles for Modal and Beam alongside
   * the execution target selector (defaulting to Local).
   *
   * Invariants:
   * 1. Dual-Provider Persistence: Both Beam and Modal profiles persist simultaneously.
   * 2. Zero Plaintext Secrets: This component manages ONLY public metadata (names, endpoints).
   *    Credentials are not collected or managed here.
   * 3. Safe Backend Error Handling: Raw backend error strings (which may contain secret URLs)
   *    are never echoed. Localized status messages are used.
   * 4. Target Integrity: Deleting the currently selected profile resets the target to Local.
   * 5. Draft Isolation: Form edits use local drafts; changes commit atomically to backend.
   */
  import { onMount } from 'svelte'
  import { Button, Field, Select, TextInput } from '../ui/index.js'
  import { getBackend } from '../api/backend.js'
  import { t } from '../i18n/index.js'

  /** @type {boolean} */
  let loading = $state(true)
  /** @type {boolean} */
  let saving = $state(false)
  /** @type {import('../api/backend.js').InferenceConfig|null} */
  let config = $state(null)

  /** @type {number} Key used to force re-mounting and re-evaluating Select when target write fails */
  let targetSelectKey = $state(0)

  /** @type {string|null} */
  let errorMessage = $state(null)
  /** @type {string|null} */
  let statusMessage = $state(null)

  /**
   * Current draft being added or edited.
   * @type {{ isNew: boolean, provider: 'modal'|'beam', id: string, name: string, endpointUrl: string, originalEndpointUrl: string, createdAtMs: number }|null}
   */
  let editingProfile = $state(null)

  /** @type {string|null} */
  let formError = $state(null)

  /** Sequence counter preventing stale async write races */
  let operationSeq = 0

  /** @type {Record<string, { checking?: boolean, ok?: boolean, latencyMs?: number, notRegistered?: boolean, message?: string }>} */
  let connectionChecks = $state({})

  /** @type {boolean} */
  let checkingRecovery = $state(false)

  /** @type {'none'|'ambiguous'|'cached'|'stale'|null} */
  let recoveryState = $state(null)

  /**
   * Compute the string key for the current selected target.
   */
  const currentTargetValue = $derived.by(() => {
    if (!config?.selectedTarget) return 'local'
    const target = config.selectedTarget
    if (target.type === 'modal' && target.profile_id) {
      return `modal:${target.profile_id}`
    }
    if (target.type === 'beam' && target.profile_id) {
      return `beam:${target.profile_id}`
    }
    return 'local'
  })

  /**
   * Compute the list of options for the execution target picker.
   */
  const targetOptions = $derived.by(() => {
    const options = [
      { value: 'local', label: t('settings.inference.target.local') },
    ]
    if (!config) return options

    for (const [id, profile] of Object.entries(config.modalProfiles || {})) {
      options.push({
        value: `modal:${id}`,
        label: t('settings.inference.target.option', {
          provider: t('settings.inference.provider.modal'),
          name: profile.name || id,
          id,
        }),
      })
    }

    for (const [id, profile] of Object.entries(config.beamProfiles || {})) {
      options.push({
        value: `beam:${id}`,
        label: t('settings.inference.target.option', {
          provider: t('settings.inference.provider.beam'),
          name: profile.name || id,
          id,
        }),
      })
    }

    return options
  })

  /**
   * Load public inference configuration from backend.
   */
  async function loadConfig() {
    const currentSeq = ++operationSeq
    loading = true
    errorMessage = null
    statusMessage = null
    try {
      const loaded = await getBackend().readInferenceConfig()
      if (operationSeq === currentSeq) {
        config = loaded
        targetSelectKey += 1
      }
    } catch {
      if (operationSeq === currentSeq) {
        errorMessage = 'settings.inference.error.loadFailed'
        config = null
      }
    } finally {
      if (operationSeq === currentSeq) {
        loading = false
      }
    }
  }

  onMount(() => {
    loadConfig()
  })

  /**
   * Change the default execution target and save to backend.
   *
   * @param {string} value
   */
  async function chooseTarget(value) {
    if (!config || saving || loading) return
    const currentSeq = ++operationSeq

    let nextTarget = { type: 'local' }
    if (value.startsWith('modal:')) {
      const profileId = value.slice('modal:'.length)
      if (config.modalProfiles && config.modalProfiles[profileId]) {
        nextTarget = { type: 'modal', profile_id: profileId }
      }
    } else if (value.startsWith('beam:')) {
      const profileId = value.slice('beam:'.length)
      if (config.beamProfiles && config.beamProfiles[profileId]) {
        nextTarget = { type: 'beam', profile_id: profileId }
      }
    }

    const nextConfig = {
      schemaVersion: config.schemaVersion ?? 1,
      selectedTarget: nextTarget,
      beamProfiles: { ...config.beamProfiles },
      modalProfiles: { ...config.modalProfiles },
    }

    saving = true
    errorMessage = null
    statusMessage = null
    try {
      const saved = await getBackend().writeInferenceConfig({ config: $state.snapshot(nextConfig) })
      if (operationSeq === currentSeq) {
        config = saved
        statusMessage = 'settings.inference.status.saved'
      }
    } catch {
      if (operationSeq === currentSeq) {
        errorMessage = 'settings.inference.error.saveFailed'
        // Force the Select component to reset and re-render with persisted currentTargetValue
        targetSelectKey += 1
      }
    } finally {
      if (operationSeq === currentSeq) {
        saving = false
      }
    }
  }

  /**
   * Start adding a new profile for a provider.
   *
   * @param {'modal'|'beam'} provider
   */
  function startAdd(provider = 'modal') {
    if (saving || loading) return
    formError = null
    errorMessage = null
    statusMessage = null
    editingProfile = {
      isNew: true,
      provider,
      id: '',
      name: '',
      endpointUrl: '',
      originalEndpointUrl: '',
      createdAtMs: Date.now(),
    }
  }

  /**
   * Start editing an existing profile.
   *
   * @param {'modal'|'beam'} provider
   * @param {import('../api/backend.js').CloudProfile} profile
   */
  function startEdit(provider, profile) {
    if (saving || loading) return
    formError = null
    errorMessage = null
    statusMessage = null
    editingProfile = {
      isNew: false,
      provider,
      id: profile.id,
      name: profile.name,
      endpointUrl: profile.endpointUrl,
      originalEndpointUrl: profile.endpointUrl,
      createdAtMs: profile.createdAtMs || Date.now(),
    }
  }

  /**
   * Cancel profile creation or editing.
   */
  function cancelEdit() {
    editingProfile = null
    formError = null
  }

  /**
   * Validate and save the current draft profile.
   */
  async function saveProfile() {
    if (!editingProfile || !config || saving || loading) return
    formError = null
    errorMessage = null
    statusMessage = null

    const { isNew, provider, id: rawId, name: rawName, endpointUrl: rawUrl, createdAtMs } = editingProfile
    const id = rawId.trim()
    const name = rawName.trim()
    const endpointUrl = rawUrl.trim()

    if (!isValidProfileId(id)) {
      formError = 'settings.inference.error.invalidId'
      return
    }

    if (isNew) {
      const targetMap = provider === 'beam' ? config.beamProfiles : config.modalProfiles
      if (targetMap && targetMap[id]) {
        formError = 'settings.inference.error.idTaken'
        return
      }
    }

    if (!isValidProfileName(name)) {
      formError = 'settings.inference.error.invalidName'
      return
    }

    if (!isValidEndpointUrl(endpointUrl)) {
      formError = 'settings.inference.error.invalidUrl'
      return
    }

    const currentSeq = ++operationSeq
    const nextBeamProfiles = { ...config.beamProfiles }
    const nextModalProfiles = { ...config.modalProfiles }

    const updatedProfile = {
      id,
      name,
      endpointUrl,
      canonicalOrigin: '',
      canonicalOriginFingerprint: '',
      createdAtMs: createdAtMs || Date.now(),
      updatedAtMs: Date.now(),
    }

    if (provider === 'beam') {
      nextBeamProfiles[id] = updatedProfile
    } else {
      nextModalProfiles[id] = updatedProfile
    }

    const nextConfig = {
      schemaVersion: config.schemaVersion ?? 1,
      selectedTarget: config.selectedTarget,
      beamProfiles: nextBeamProfiles,
      modalProfiles: nextModalProfiles,
    }

    saving = true
    try {
      const saved = await getBackend().writeInferenceConfig({ config: $state.snapshot(nextConfig) })
      if (operationSeq === currentSeq) {
        config = saved
        editingProfile = null
        statusMessage = 'settings.inference.status.saved'
        targetSelectKey += 1
      }
    } catch {
      if (operationSeq === currentSeq) {
        errorMessage = 'settings.inference.error.saveFailed'
      }
    } finally {
      if (operationSeq === currentSeq) {
        saving = false
      }
    }
  }

  /**
   * Delete a profile from a provider and persist to backend.
   * If the deleted profile was the selected target, resets selectedTarget to local.
   *
   * @param {'modal'|'beam'} provider
   * @param {string} profileId
   */
  async function deleteProfile(provider, profileId) {
    if (!config || saving || loading) return
    const currentSeq = ++operationSeq

    const nextBeamProfiles = { ...config.beamProfiles }
    const nextModalProfiles = { ...config.modalProfiles }

    if (provider === 'beam') {
      delete nextBeamProfiles[profileId]
    } else if (provider === 'modal') {
      delete nextModalProfiles[profileId]
    }

    let nextTarget = config.selectedTarget
    if (
      nextTarget &&
      nextTarget.type === provider &&
      nextTarget.profile_id === profileId
    ) {
      nextTarget = { type: 'local' }
    }

    const nextConfig = {
      schemaVersion: config.schemaVersion ?? 1,
      selectedTarget: nextTarget,
      beamProfiles: nextBeamProfiles,
      modalProfiles: nextModalProfiles,
    }

    saving = true
    errorMessage = null
    statusMessage = null
    try {
      const saved = await getBackend().writeInferenceConfig({ config: $state.snapshot(nextConfig) })
      if (operationSeq === currentSeq) {
        config = saved
        if (
          editingProfile &&
          editingProfile.provider === provider &&
          editingProfile.id === profileId
        ) {
          editingProfile = null
        }
        statusMessage = 'settings.inference.status.saved'
        targetSelectKey += 1
      }
    } catch {
      if (operationSeq === currentSeq) {
        errorMessage = 'settings.inference.error.saveFailed'
      }
    } finally {
      if (operationSeq === currentSeq) {
        saving = false
      }
    }
  }

  /**
   * Test control-plane reachability for a profile.
   * Ordinary connection check: strictly reachability/auth, never triggers GPU work.
   *
   * @param {'modal'|'beam'} provider
   * @param {string} profileId
   */
  async function checkConnection(provider, profileId) {
    connectionChecks[profileId] = { checking: true }
    try {
      const res = await getBackend().checkCloudConnection({ provider, profileId })
      if (res && res.ok) {
        connectionChecks[profileId] = { ok: true, latencyMs: res.latencyMs }
      } else {
        connectionChecks[profileId] = { ok: false, message: res?.message }
      }
    } catch (err) {
      const msg = String(err?.message ?? err ?? '')
      if (msg.includes('not found') || msg.includes('not registered') || msg.includes('was constructed outside')) {
        connectionChecks[profileId] = { notRegistered: true }
      } else {
        connectionChecks[profileId] = { ok: false }
      }
    }
  }

  /**
   * Reconcile interrupted attempt recovery state.
   */
  async function checkRecovery() {
    checkingRecovery = true
    try {
      const res = await getBackend().reconcileCloudRecovery()
      if (!res || res.decision === 'terminal' || res.decision === 'already_committed') {
        recoveryState = 'none'
      } else if (res.decision === 'ambiguous_unknown') {
        recoveryState = 'ambiguous'
      } else if (res.decision === 'result_cached_ready') {
        recoveryState = 'cached'
      } else if (res.decision === 'stale_attachment') {
        recoveryState = 'stale'
      } else {
        recoveryState = 'none'
      }
    } catch {
      recoveryState = 'none'
    } finally {
      checkingRecovery = false
    }
  }
</script>

<div class="inference-settings">
  <div class="disclaimer-box">
    <p class="disclaimer-text">{t('settings.inference.disclaimer')}</p>
    <p class="execution-status-text">
      <strong>{t('settings.inference.executionStatus.label')}:</strong> {t('settings.inference.executionStatus.unavailable')}
    </p>
  </div>

  {#if loading}
    <p class="note">{t('settings.inference.loading')}</p>
  {:else if errorMessage && !config}
    <div class="load-error-wrap">
      <div class="status-banner error" role="alert">
        {t(errorMessage)}
      </div>
      <Button size="sm" onclick={loadConfig}>
        {t('home.action.retry')}
      </Button>
    </div>
  {:else if config}
    <div class="target-field-wrap">
      <Field label={t('settings.inference.target.label')} layout="row">
        {#snippet children({ labelId })}
          {#key targetSelectKey}
            <Select
              options={targetOptions}
              value={currentTargetValue}
              labelledBy={labelId}
              disabled={saving || loading}
              onchange={chooseTarget}
            />
          {/key}
        {/snippet}
      </Field>
    </div>

    {#if editingProfile}
      <div
        class="profile-form-card"
        role="region"
        aria-label={editingProfile.isNew
          ? t('settings.inference.addProfile')
          : t('settings.inference.editProfile')}
      >
        <div class="form-header">
          <h4 class="form-title">
            {editingProfile.isNew
              ? t('settings.inference.addProfile')
              : t('settings.inference.editProfile')}
          </h4>
        </div>

        {#if editingProfile.isNew}
          <div class="form-field-wrap">
            <Field
              label={t('settings.inference.provider.label')}
              layout="stack"
              controlId="inference-provider-select"
            >
              {#snippet children()}
                <select
                  id="inference-provider-select"
                  class="sidecar-model-select"
                  value={editingProfile.provider}
                  disabled={saving}
                  onchange={(e) => {
                    if (editingProfile) {
                      editingProfile = {
                        ...editingProfile,
                        provider: /** @type {'modal'|'beam'} */ (e.currentTarget.value),
                      }
                    }
                  }}
                >
                  <option value="modal">{t('settings.inference.provider.modal')}</option>
                  <option value="beam">{t('settings.inference.provider.beam')}</option>
                </select>
              {/snippet}
            </Field>
          </div>

          <div class="form-field-wrap">
            <Field
              label={t('settings.inference.id.label')}
              layout="stack"
              controlId="inference-profile-id"
            >
              {#snippet children()}
                <TextInput
                  id="inference-profile-id"
                  value={editingProfile.id}
                  disabled={saving}
                  placeholder={t('settings.inference.id.placeholder')}
                  onchange={(val) => {
                    if (editingProfile) {
                      editingProfile = { ...editingProfile, id: val }
                    }
                  }}
                />
              {/snippet}
            </Field>
          </div>
        {/if}

        <div class="form-field-wrap">
          <Field
            label={t('settings.inference.name.label')}
            layout="stack"
            controlId="inference-profile-name"
          >
            {#snippet children()}
              <TextInput
                id="inference-profile-name"
                value={editingProfile.name}
                disabled={saving}
                placeholder={t('settings.inference.name.placeholder')}
                onchange={(val) => {
                  if (editingProfile) {
                    editingProfile = { ...editingProfile, name: val }
                  }
                }}
              />
            {/snippet}
          </Field>
        </div>

        <div class="form-field-wrap">
          <Field
            label={t('settings.inference.endpoint.label')}
            layout="stack"
            controlId="inference-profile-endpoint"
          >
            {#snippet children()}
              <TextInput
                id="inference-profile-endpoint"
                value={editingProfile.endpointUrl}
                disabled={saving}
                placeholder={t('settings.inference.endpoint.placeholder')}
                onchange={(val) => {
                  if (editingProfile) {
                    editingProfile = { ...editingProfile, endpointUrl: val }
                  }
                }}
              />
            {/snippet}
          </Field>
        </div>

        {#if !editingProfile.isNew && isOriginChanged(editingProfile.originalEndpointUrl, editingProfile.endpointUrl)}
          <div class="warning-note" role="alert">
            {t('settings.inference.originWarning')}
          </div>
        {/if}

        {#if formError}
          <div class="form-error" role="alert">
            {t(formError)}
          </div>
        {/if}

        <div class="form-actions">
          <Button variant="primary" onclick={saveProfile} disabled={saving}>
            {t('settings.inference.save')}
          </Button>
          <Button onclick={cancelEdit} disabled={saving}>
            {t('settings.inference.cancel')}
          </Button>
        </div>
      </div>
    {/if}

    <div class="provider-section">
      <div class="section-head">
        <h4 class="section-title">{t('settings.inference.modalSection')}</h4>
        <Button
          size="sm"
          onclick={() => startAdd('modal')}
          disabled={saving || !!editingProfile}
        >
          {t('settings.inference.addProfile')}
        </Button>
      </div>

      {#if Object.keys(config.modalProfiles || {}).length === 0}
        <p class="empty-note">{t('settings.inference.noProfiles')}</p>
      {:else}
        <ul class="rows">
          {#each Object.entries(config.modalProfiles) as [id, profile] (id)}
            <li class="row">
              <div class="row-text">
                <span class="row-name">
                  {profile.name} <span class="profile-id">({profile.id})</span>
                </span>
                <span class="row-meta">{profile.endpointUrl}</span>
                {#if connectionChecks[id]}
                  <div class="connection-status" role="status" aria-live="polite">
                    {#if connectionChecks[id].checking}
                      <span class="status-indicator checking">{t('settings.inference.testingConnection')}</span>
                    {:else if connectionChecks[id].ok}
                      <span class="status-indicator ok">{t('settings.inference.connectionReachable', { latency: connectionChecks[id].latencyMs ?? 0 })}</span>
                    {:else if connectionChecks[id].notRegistered}
                      <span class="status-indicator unavail">{t('settings.inference.commandNotRegistered')}</span>
                    {:else}
                      <span class="status-indicator failed">{t('settings.inference.connectionFailed')}</span>
                    {/if}
                  </div>
                {/if}
              </div>
              <div class="row-actions">
                <Button
                  size="sm"
                  aria-label={t("settings.inference.testNamed", { name: profile.name, id })}
                  onclick={() => checkConnection('modal', id)}
                  disabled={saving || connectionChecks[id]?.checking}
                >
                  {t('settings.inference.testConnection')}
                </Button>
                <Button
                  size="sm"
                  aria-label={t("settings.inference.editNamed", { name: profile.name, id })}
                  onclick={() => startEdit('modal', profile)}
                  disabled={saving || !!editingProfile}
                >
                  {t('settings.inference.edit')}
                </Button>
                <Button
                  size="sm"
                  aria-label={t("settings.inference.deleteNamed", { name: profile.name, id })}
                  onclick={() => deleteProfile('modal', id)}
                  disabled={saving}
                >
                  {t('settings.inference.deleteProfile')}
                </Button>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </div>

    <div class="provider-section">
      <div class="section-head">
        <h4 class="section-title">{t('settings.inference.beamSection')}</h4>
        <Button
          size="sm"
          onclick={() => startAdd('beam')}
          disabled={saving || !!editingProfile}
        >
          {t('settings.inference.addProfile')}
        </Button>
      </div>

      {#if Object.keys(config.beamProfiles || {}).length === 0}
        <p class="empty-note">{t('settings.inference.noProfiles')}</p>
      {:else}
        <ul class="rows">
          {#each Object.entries(config.beamProfiles) as [id, profile] (id)}
            <li class="row">
              <div class="row-text">
                <span class="row-name">
                  {profile.name} <span class="profile-id">({profile.id})</span>
                </span>
                <span class="row-meta">{profile.endpointUrl}</span>
                {#if connectionChecks[id]}
                  <div class="connection-status" role="status" aria-live="polite">
                    {#if connectionChecks[id].checking}
                      <span class="status-indicator checking">{t('settings.inference.testingConnection')}</span>
                    {:else if connectionChecks[id].ok}
                      <span class="status-indicator ok">{t('settings.inference.connectionReachable', { latency: connectionChecks[id].latencyMs ?? 0 })}</span>
                    {:else if connectionChecks[id].notRegistered}
                      <span class="status-indicator unavail">{t('settings.inference.commandNotRegistered')}</span>
                    {:else}
                      <span class="status-indicator failed">{t('settings.inference.connectionFailed')}</span>
                    {/if}
                  </div>
                {/if}
              </div>
              <div class="row-actions">
                <Button
                  size="sm"
                  aria-label={t("settings.inference.testNamed", { name: profile.name, id })}
                  onclick={() => checkConnection('beam', id)}
                  disabled={saving || connectionChecks[id]?.checking}
                >
                  {t('settings.inference.testConnection')}
                </Button>
                <Button
                  size="sm"
                  aria-label={t("settings.inference.editNamed", { name: profile.name, id })}
                  onclick={() => startEdit('beam', profile)}
                  disabled={saving || !!editingProfile}
                >
                  {t('settings.inference.edit')}
                </Button>
                <Button
                  size="sm"
                  aria-label={t("settings.inference.deleteNamed", { name: profile.name, id })}
                  onclick={() => deleteProfile('beam', id)}
                  disabled={saving}
                >
                  {t('settings.inference.deleteProfile')}
                </Button>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </div>

    <div class="recovery-section">
      <div class="section-head">
        <h4 class="section-title">{t('settings.inference.recovery.title')}</h4>
        <Button
          size="sm"
          onclick={checkRecovery}
          disabled={saving || checkingRecovery}
        >
          {t('settings.inference.recovery.check')}
        </Button>
      </div>

      {#if recoveryState}
        <div class="recovery-status-box" role="status" aria-live="polite">
          {#if recoveryState === 'ambiguous'}
            <p class="recovery-text">{t('settings.inference.recovery.ambiguous')}</p>
          {:else if recoveryState === 'cached'}
            <p class="recovery-text">{t('settings.inference.recovery.cached')}</p>
          {:else if recoveryState === 'stale'}
            <p class="recovery-text">{t('settings.inference.recovery.stale')}</p>
          {:else}
            <p class="recovery-text">{t('settings.inference.recovery.none')}</p>
          {/if}
        </div>
      {/if}
    </div>
  {/if}

  {#if errorMessage && config}
    <div class="status-banner error" role="alert">
      {t(errorMessage)}
    </div>
  {/if}

  {#if statusMessage}
    <div class="status-banner success" role="status">
      {t(statusMessage)}
    </div>
  {/if}
</div>

<style>
  .inference-settings {
    display: flex;
    flex-direction: column;
    gap: var(--s-3);
  }

  .disclaimer-box {
    padding: var(--s-2) var(--s-3);
    border: 1px solid var(--line2);
    border-radius: var(--r-md);
    background: var(--surface);
  }

  .disclaimer-text {
    margin: 0;
    font-size: 11px;
    color: var(--t2);
    line-height: 1.45;
  }

  .execution-status-text {
    margin: var(--s-1) 0 0 0;
    font-size: 11px;
    color: var(--t2);
    line-height: 1.4;
  }

  .connection-status {
    margin-top: 2px;
  }

  .status-indicator {
    font-size: 10px;
    line-height: 1.4;
  }

  .status-indicator.checking {
    color: var(--t2);
  }

  .status-indicator.ok {
    color: #10b981;
  }

  .status-indicator.failed {
    color: var(--warn);
  }

  .status-indicator.unavail {
    color: var(--t3);
  }

  .recovery-section {
    display: flex;
    flex-direction: column;
    padding-top: var(--s-2);
    border-top: 1px solid var(--line);
  }

  .recovery-status-box {
    padding: var(--s-2) var(--s-3);
    border-radius: var(--r-md);
    background: var(--surface);
    border: 1px solid var(--line2);
    margin-top: var(--s-1);
  }

  .recovery-text {
    margin: 0;
    font-size: 11px;
    color: var(--t2);
    line-height: 1.45;
  }

  .target-field-wrap {
    padding-bottom: var(--s-2);
  }

  .provider-section {
    display: flex;
    flex-direction: column;
    padding-top: var(--s-2);
    border-top: 1px solid var(--line);
  }

  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: var(--s-2);
  }

  .section-title {
    margin: 0;
    font-size: 12px;
    font-weight: 600;
    color: var(--text);
  }

  .empty-note {
    margin: var(--s-1) 0;
    font-size: 11px;
    color: var(--t3);
  }

  .profile-id {
    font-size: 10.5px;
    color: var(--t3);
    font-family: monospace;
  }

  .profile-form-card {
    display: flex;
    flex-direction: column;
    gap: var(--s-2);
    padding: var(--s-3);
    border: 1px solid var(--line2);
    border-radius: var(--r-md);
    background: var(--panel2);
    margin: var(--s-2) 0;
  }

  .form-header {
    margin-bottom: var(--s-1);
  }

  .form-title {
    margin: 0;
    font-size: 12px;
    font-weight: 600;
    color: var(--text);
  }

  .form-field-wrap {
    margin-bottom: var(--s-1);
  }

  .warning-note {
    padding: var(--s-2);
    border-radius: var(--r-sm);
    background: var(--surface);
    color: var(--warn);
    font-size: 10.5px;
    line-height: 1.4;
  }

  .form-error {
    color: var(--warn);
    font-size: 11px;
    line-height: 1.4;
  }

  .form-actions {
    display: flex;
    gap: var(--s-2);
    margin-top: var(--s-2);
  }

  .load-error-wrap {
    display: flex;
    flex-direction: column;
    gap: var(--s-2);
    align-items: flex-start;
  }

  .status-banner {
    padding: var(--s-2) var(--s-3);
    border-radius: var(--r-sm);
    font-size: 11px;
    line-height: 1.4;
  }

  .status-banner.error {
    color: var(--warn);
    background: var(--surface);
  }

  .status-banner.success {
    color: var(--text);
    background: var(--surface);
  }

  .rows {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    padding: var(--s-2) 0;
    border-bottom: 1px solid var(--line);
  }

  .row:last-child {
    border-bottom: none;
  }

  .row-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }

  .row-name {
    font-size: 12px;
    color: var(--text);
  }

  .row-meta {
    font-size: 10.5px;
    color: var(--t3);
    line-height: 1.4;
    word-break: break-all;
  }

  .row-actions {
    display: flex;
    flex: none;
    gap: var(--s-2);
  }

  .note {
    margin: 0 0 var(--s-2);
    font-size: 10.5px;
    color: var(--t3);
    line-height: 1.45;
  }

  .sidecar-model-select {
    width: 100%;
    height: 28px;
    padding: 0 var(--s-2);
    border: 1px solid var(--line2);
    border-radius: var(--r-md);
    background: var(--panel);
    color: var(--text);
    font: inherit;
    font-size: 11.5px;
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease),
      border-color var(--dur-fast) var(--ease);
  }

  .sidecar-model-select:hover:not(:disabled) {
    border-color: var(--accent);
  }

  .sidecar-model-select:focus-visible {
    outline: none;
    border-color: var(--accent);
  }

  .sidecar-model-select:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
