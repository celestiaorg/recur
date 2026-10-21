<script lang="ts">
  import { SettingsService, bridgeErrorMessage, callOk } from '../lib/api'
  import { settings, pushToast } from '../lib/stores'
  import { openExternalUrl } from '../lib/open-external'
  import { invalidateModelCatalog } from '../lib/models'
  import type { ChatGPTStatus } from '../../bindings/recur/internal/app/models'
  import Select from './Select.svelte'

  let { onstatus }: { onstatus: (status: ChatGPTStatus) => void } = $props()
  let status = $state.raw<ChatGPTStatus | null>(null)
  let error = $state('')
  let loading = $state(true)
  let starting = $state(false)
  let loginID = $state('')
  let authURL = $state('')
  let userCode = $state('')
  let loginMethod = $state('browser')
  let selectedAccount = $state('')
  let attempt = 0
  const signingIn = $derived(starting || !!loginID)
  const accounts = $derived([
    ...(status?.accounts ?? []).map((a) => ({ value: a.id, label: `${a.email || 'ChatGPT account'} (${a.id})` })),
    { value: '', label: 'Another ChatGPT account' },
  ])

  function cancelLogin() {
    ++attempt
    const id = loginID
    starting = false
    loginID = ''
    authURL = ''
    userCode = ''
    if (id) void callOk(SettingsService.CancelChatGPTLogin(id), 'Could not cancel ChatGPT sign-in')
  }

  $effect(() => {
    let disposed = false
    SettingsService.GetChatGPTStatus()
      .then((next) => {
        if (disposed) return
        status = next
        selectedAccount = next.accountId || next.accounts?.[0]?.id || ''
        onstatus(next)
      })
      .catch((err) => { if (!disposed) error = bridgeErrorMessage(err) || 'Could not read ChatGPT account' })
      .finally(() => { if (!disposed) loading = false })
    return () => { disposed = true; cancelLogin() }
  })

  async function applyStatus(next: ChatGPTStatus, current: number) {
    const saved = await SettingsService.GetSettings()
    if (current !== attempt) return
    settings.set(saved)
    status = next
    selectedAccount = next.accountId || next.accounts?.[0]?.id || ''
    invalidateModelCatalog()
    onstatus(next)
  }

  async function signIn() {
    const current = ++attempt
    starting = true
    error = ''
    let id = ''
    try {
      const login = await SettingsService.StartChatGPTLogin(selectedAccount, loginMethod)
      id = login.loginId
      if (current !== attempt) { await SettingsService.CancelChatGPTLogin(id); return }
      loginID = id
      authURL = login.authUrl
      userCode = login.userCode || ''
      starting = false
      if (login.method === 'browser') await openSignInPage()
      const next = await SettingsService.WaitChatGPTLogin(id)
      await applyStatus(next, current)
      if (current === attempt) pushToast('success', 'ChatGPT connected')
    } catch (err) {
      if (current === attempt) {
        error = bridgeErrorMessage(err) || 'Could not connect ChatGPT'
        // OAuth may have completed while model discovery failed. Keep that
        // account visible so Refresh models or reauthorization can recover.
        await SettingsService.GetChatGPTStatus().then((next) => {
          if (current !== attempt) return
          status = next
          selectedAccount = next.accountId || next.accounts?.[0]?.id || ''
          onstatus(next)
        }).catch(() => {})
      }
      if (id) await SettingsService.CancelChatGPTLogin(id).catch(() => {})
    } finally {
      if (current === attempt) { starting = false; loginID = ''; authURL = ''; userCode = '' }
    }
  }

  async function openSignInPage() {
    try {
      error = ''
      await openExternalUrl(authURL)
    } catch (err) {
      error = `${bridgeErrorMessage(err) || 'Could not open your browser'}. Open the sign-in link manually to continue.`
    }
  }

  async function updateAccount(disconnect: boolean) {
    const current = ++attempt
    loading = true
    error = ''
    try {
      const next = await (disconnect ? SettingsService.DisconnectChatGPT() : SettingsService.RefreshChatGPTModels())
      await applyStatus(next, current)
    } catch (err) {
      if (current === attempt) error = bridgeErrorMessage(err) || 'Could not update ChatGPT account'
    } finally {
      if (current === attempt) loading = false
    }
  }
</script>

<div class="set-rows code-panel-sides">
  <div class="set-row">
    <div class="set-copy"><div class="set-title">Sign-in method</div><div class="set-desc">Use a browser callback, or a headless device code that works from any browser.</div></div>
    <div class="set-ctrl"><Select value={loginMethod} options={[{ value: 'browser', label: 'Browser' }, { value: 'headless', label: 'Headless (device code)' }]} onchange={(method) => { loginMethod = method }} disabled={loading || signingIn} label="ChatGPT sign-in method" /></div>
  </div>
  <div class="set-row">
    <div class="set-copy">
      <div class="set-title">{status?.authenticated ? 'Connected' : 'ChatGPT account'}</div>
      <div class="set-desc">
        {#if starting}
          Starting sign-in…
        {:else if loginID}
          {userCode ? 'Open the sign-in link in any browser and enter the one-time code. Recur will connect automatically.' : 'Complete sign-in in your browser.'}
        {:else if loading}
          Checking ChatGPT account…
        {:else if status?.authenticated}
          {status.email || 'Signed in'} · {status.modelCount} {status.modelCount === 1 ? 'model' : 'models'} available
        {:else}
          Use your ChatGPT plan directly. Recur runs the coding tools. No Codex installation or API key is needed.
        {/if}
      </div>
      {#if error || status?.error}
        <div class="set-field-msg bad" role="alert">{error || status?.error}</div>
      {/if}
    </div>
    <div class="set-ctrl">
      {#if signingIn}
        {#if authURL}<button class="tbtn" type="button" onclick={openSignInPage}>Open sign-in page</button>{/if}
        <button class="tbtn" type="button" onclick={cancelLogin}>Cancel ChatGPT sign-in</button>
      {:else}
        <button class="tbtn" type="button" disabled={loading} onclick={signIn}>Continue with ChatGPT</button>
      {/if}
    </div>
  </div>
  {#if loginID && authURL}
    <div class="set-row set-row-stack">
      <div class="set-copy"><div class="set-title">Sign-in link</div></div>
      <input class="inp" aria-label="ChatGPT sign-in link" readonly value={authURL} onclick={(event) => event.currentTarget.select()} />
      {#if userCode}
        <div class="set-desc">One-time code: <code aria-label="ChatGPT device code">{userCode}</code>. Expires in 15 minutes.</div>
      {/if}
    </div>
  {/if}
  {#if (status?.accounts?.length ?? 0) > 0}
    <div class="set-row">
      <div class="set-copy"><div class="set-title">Sign-in account</div><div class="set-desc">Sign in to a saved account or connect another ChatGPT account.</div></div>
      <div class="set-ctrl"><Select options={accounts} value={selectedAccount} onchange={(value) => { selectedAccount = value }} disabled={signingIn || loading} label="ChatGPT sign-in account" /></div>
    </div>
  {/if}
  {#if status?.authenticated}
    <div class="set-row">
      <div class="set-copy"><div class="set-title">Account access</div><div class="set-desc">Credentials are stored locally in Recur’s data directory.</div></div>
      <div class="set-ctrl">
        <button class="tbtn" type="button" disabled={loading || signingIn} onclick={() => updateAccount(false)}>Refresh ChatGPT models</button>
        <button class="tbtn" type="button" disabled={loading || signingIn} onclick={() => updateAccount(true)}>Sign out of ChatGPT</button>
      </div>
    </div>
  {/if}
</div>
