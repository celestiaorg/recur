<script lang="ts">
  // Inline ask_user / loop-escalation answer UI in the chat transcript.
  // Off-tab ask_user notice is a toast (QuestionPrompts), not a second card.
  // Complete options submit on click. Options tagged `notes` select first;
  // Answer sends choice+text (the loop's "Proceed with my notes" plan-approval).
  import { answerQuestion } from '../lib/api'
  import { optionChoiceText } from '../lib/agentchat'
  import Icon from './Icon.svelte'
  import MarkdownHtml from './MarkdownHtml.svelte'

  type QuestionOpt = { text: string; tag?: string; desc?: string }
  type QuestionLike = {
    id: string
    question: string
    context?: string
    options?: QuestionOpt[]
  }

  let {
    question,
    kicker = 'Question for you',
    onAnswer,
  }: {
    question: QuestionLike
    kicker?: string
    onAnswer?: (choice: string, text: string) => void
  } = $props()

  let freeText = $state('')
  let notesOpt = $state<QuestionOpt | null>(null)
  let notesEl = $state<HTMLTextAreaElement | undefined>(undefined)

  const options = $derived(question.options ?? [])
  const context = $derived((question.context ?? '').trim())
  const notesPending = $derived(!!notesOpt && optionNeedsInput(notesOpt))
  const canAnswer = $derived(!!freeText.trim())

  function optionNeedsInput(opt: QuestionOpt) {
    return opt.tag === 'notes'
  }

  function submit(choice = '', text = freeText.trim()) {
    const sent = optionChoiceText(choice)
    if (onAnswer) onAnswer(sent, text)
    else void answerQuestion(question.id, sent, text)
  }

  function pick(opt: QuestionOpt) {
    if (optionNeedsInput(opt)) {
      notesOpt = opt
      queueMicrotask(() => notesEl?.focus())
      return
    }
    notesOpt = null
    submit(opt.text)
  }

  function answerFree() {
    const text = freeText.trim()
    if (!text) return
    submit(notesPending ? (notesOpt?.text ?? '') : '', text)
  }

  function onFreeKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && freeText.trim()) {
      e.preventDefault()
      answerFree()
    }
  }
</script>

<div class="perm inline" role="group" aria-label={kicker}>
  <div class="perm-head">
    <Icon name="message-question" class="perm-ico" />
    <div class="perm-head-text">
      <span class="perm-kicker">{kicker}</span>
    </div>
  </div>

  <div class="perm-main">
    <div class="perm-tool ask-q">
      <MarkdownHtml text={question.question} class="chat-canvas" />
    </div>
    {#if context}
      <MarkdownHtml text={context} class="chat-canvas perm-sum" />
    {/if}
  </div>

  {#if options.length}
    <div class="ask-opts">
      {#each options as opt (opt.text)}
        {@const needs = optionNeedsInput(opt)}
        <button
          type="button"
          class="perm-btn ask-opt"
          class:sel={needs && notesOpt?.text === opt.text}
          title={opt.desc || opt.text}
          aria-pressed={needs ? notesOpt?.text === opt.text : undefined}
          onclick={() => pick(opt)}
        >
          <span class="ask-opt-text">
            <MarkdownHtml text={opt.text} class="chat-canvas" />
          </span>
          {#if opt.desc}
            <span class="ask-opt-desc">{opt.desc}</span>
          {/if}
        </button>
      {/each}
    </div>
  {/if}

  <div class="ask-free" class:notes={notesPending}>
    {#if notesPending}
      <textarea
        class="ask-input ask-notes"
        rows="3"
        placeholder="Add your notes…"
        bind:this={notesEl}
        bind:value={freeText}
        onkeydown={onFreeKey}
      ></textarea>
    {:else}
      <input
        class="ask-input"
        type="text"
        placeholder={options.length ? 'Or type your own answer…' : 'Your answer…'}
        bind:value={freeText}
        onkeydown={onFreeKey}
      />
    {/if}
    <button
      type="button"
      class="perm-btn go"
      disabled={!canAnswer}
      onclick={answerFree}
    >Answer</button>
  </div>
</div>

<style>
  :global(.perm.inline) .ask-q {
    font-family: var(--ui);
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text);
    letter-spacing: -0.01em;
  }
  :global(.perm.inline .ask-q .chat-canvas),
  :global(.perm.inline .ask-opt-text .chat-canvas) {
    font: inherit;
    font-size: inherit;
    color: inherit;
  }
  :global(.perm.inline .ask-q .chat-canvas p),
  :global(.perm.inline .ask-opt-text .chat-canvas p) {
    margin: 0;
  }

  :global(.perm.inline .perm-sum.chat-canvas) {
    font-size: 12.5px;
    line-height: 1.55;
    max-height: min(60vh, 560px);
    overflow: auto;
  }

  .ask-opts {
    display: flex;
    flex-direction: column;
    gap: 5px;
    margin-top: 9px;
  }
  .ask-opt {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
    text-align: left;
    white-space: normal;
    padding: 5px 8px;
    width: 100%;
  }
  .ask-opt-text {
    font-size: 12px;
    font-weight: 550;
    color: var(--text);
  }
  .ask-opt-desc {
    font-size: 11px;
    color: var(--muted);
    line-height: 1.35;
  }
  .ask-opt:hover .ask-opt-text {
    color: var(--text);
  }
  .ask-opt.sel {
    border-color: var(--amber);
    background: color-mix(in srgb, var(--amber) 10%, var(--pane));
  }

  .ask-free {
    display: flex;
    gap: 6px;
    align-items: center;
    margin-top: 9px;
  }
  .ask-free.notes {
    align-items: stretch;
  }
  .ask-input {
    flex: 1;
    min-width: 0;
    border: 1px solid var(--line-bright);
    border-radius: 6px;
    background: var(--bg);
    color: var(--text);
    font: inherit;
    font-size: 12px;
    padding: 6px 8px;
  }
  .ask-notes {
    resize: vertical;
    min-height: 4.2em;
    line-height: 1.4;
  }
  .ask-input:focus {
    outline: none;
    border-color: color-mix(in srgb, var(--amber) 55%, var(--line));
  }
  .ask-free .perm-btn.go {
    flex: none;
    padding-inline: 12px;
    align-self: flex-end;
  }
  .ask-free .perm-btn.go:disabled {
    opacity: 0.45;
    cursor: default;
    filter: none;
  }
</style>
