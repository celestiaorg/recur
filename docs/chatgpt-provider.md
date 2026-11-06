# OpenAI ChatGPT sign-in: browser and headless

**Settings → Providers → OpenAI → ChatGPT account → Continue with ChatGPT** connects a ChatGPT
account directly to Recur. No Codex installation, CLI login, or API key is needed.
Choose a model in the **OpenAI** group (`chatgpt/<model-slug>`) to use it for a
chat or loop. Recur owns the agent loop, permissions, and coding tools.

## Authentication and storage

This follows the Codex-compatible browser/device flows used by
[OpenCode](https://github.com/anomalyco/opencode/blob/dev/packages/core/src/plugin/provider/openai.ts),
using public client ID `app_EMoamEEZ73f0CkXaXp7hrann`. It requests only
`openid profile email offline_access`, without dynamic agent registration,
host IDs, or agent-plan permission scopes. Requests identify Recur as `recur`.

Choose **Sign-in method** before clicking **Continue with ChatGPT**:

- **Browser** opens `https://auth.openai.com/oauth/authorize` with fresh PKCE and
  state, `id_token_add_organizations=true`, and `codex_cli_simplified_flow=true`.
  The callback is `http://localhost:1455/auth/callback`, falling back to port 1457
  if 1455 is busy. Recur does not cancel other applications' pending logins.
  The complete sign-in link remains visible for manual opening/copying.
- **Headless (device code)** displays `https://auth.openai.com/codex/device` and
  a one-time code. Open that link on any browser/device and enter the code.
  Recur polls for completion without a local callback listener or automatic
  browser launch. **Open sign-in page** explicitly opens the link if desired.

Both methods exchange authorization codes at `https://auth.openai.com/oauth/token`.
Recur verifies the ID-token signature, issuer, audience, expiry, and selected
identity. The account and rotating tokens are saved atomically in
`<RECUR_DATA_DIR>/chatgpt-auth.json` with owner-only permissions. Tokens are not
part of Settings or returned to the UI. The ChatGPT account/workspace ID
is sent with the bearer token when fetching models or performing inference.

Browser attempts expire after five minutes, device-code attempts after fifteen.
Cancel, leaving Providers, and app shutdown close the listener or stop polling.
A failed attempt keeps the active account. Saved accounts appear in **Sign-in
account**; choose one to sign in again or **Another ChatGPT account** to switch.

Access tokens refresh automatically near expiry. Settings and all agent turns
share the refresh client so concurrent turns cannot race a rotating refresh
token. **Refresh ChatGPT models** refreshes the selected account's catalogue.
**Sign out of ChatGPT** clears the selected account's local tokens, retaining
account metadata for later sign-in, like OpenCode sign-out.

After upgrading from the earlier agent-registration implementation, sign in once
using Browser or Headless. Its previous agent-flow tokens are not reused for the
Codex subscription endpoint.

Recur does not read or modify OpenCode's credentials or `~/.codex` for this provider.

Usage: with the provider enabled and an account signed in, the sidebar usage meter
and popover show a **ChatGPT** row with 5-hour and 7-day bars. The row is removed
when ChatGPT is disabled or signed out, and shows an error when the login has expired.

## Inference

```
Recur → HTTPS chatgpt.com/backend-api/codex/responses
        OAuth bearer token
        store:false, stream:true
        Recur tool loop and transcript history
```

The model catalogue comes from `https://chatgpt.com/backend-api/codex/models`
with the Codex protocol compatibility version. Only models with
`visibility: "list"` are offered, preserving server order.

`providers/compatible/src/chatgpt.rs` applies the Codex route's streaming/stateless
request contract and omits unsupported fields. Function tools keep their normal
schemas. Encrypted reasoning and model output are replayed between tool rounds.
Account switching refreshes the model list; an existing runner is bound to its
original account so it cannot silently switch identities. Recur runs its local
coding and workspace tools in-process.

| File | Role |
|---|---|
| `providers/chatgpt/src` | Browser PKCE, device polling, ID-token verification, credentials, refresh, models, the provider |
| `crates/services/src/app/settings.rs` | Settings bridge and account-specific model cache |
| `providers/compatible/src/harness.rs` | Recur tool loop and streamed output |
| `providers/compatible/src/chatgpt.rs` | ChatGPT request adapter |
| `crates/views/settings/src/sections/openai.rs` | Account settings and browser sign-in |

### Live verification

1. Open **Settings → Providers → OpenAI → ChatGPT account**, then **Continue
   with ChatGPT**. Codex runtime and API endpoint setup are in the same OpenAI section.
2. For Browser, complete ordinary ChatGPT sign-in. For Headless, confirm the
   device link/code appears without launching a browser, then enter the code at
   that link. The flow should not ask to register Recur as an agent.
3. Confirm your email and model count. Select a model from the **OpenAI** group
   in Settings or the chat model picker.
4. Add a scratch project and send a short prompt. In Agent mode, ask it to read
   or create a scratch file to exercise a local tool call.
5. Quit the test instance and restart with the same command. Confirm the account
   remains connected and another prompt works.
6. Test cancellation and navigation away during sign-in. Then test sign-out and
   sign-in to a saved account.

The live sign-in and inference steps require your own ChatGPT account. Automated
tests use a signed local OAuth server and a streamed Responses fixture; they do
not use a real account or require Codex.

### Automated checks

```sh
mise run check
cargo test -p recur-provider-chatgpt -p recur-provider-compatible
cargo test -p recur-view-settings openai
```
