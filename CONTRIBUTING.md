# Contributing to Recur

Thanks for helping. [`AGENTS.md`](AGENTS.md) covers the codebase conventions and quality gates, and [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) the crate layout. To get set up, install [mise](https://mise.jdx.dev), then run `mise install` and `mise run dev`.

## Reporting bugs

Open an issue with the Recur version, your platform, steps to reproduce, and any logs, screenshots or recordings.

## What's welcome

- Documentation fixes and improvements
- Fixes for reported bugs, especially ones with reproduction steps
- New bundles and improvements to the shipped ones
- Small enhancements to existing features

## Before you start

- **Discuss features first.** Open an issue and get a maintainer's go-ahead before writing a feature. Bug fixes with clear reproduction steps can go straight to a PR.
- **Prefer a bundle.** Language support, commands, snippets and many workflows belong in a [Recur Bundle](docs/bundles.md), not in core.
- **Keep PRs small and focused.** One change per PR. Giant refactors, style-only churn and non-trivial changes without tests are unlikely to be merged.

## AI policy

AI-assisted work is welcome, but a human has to be in the loop: you understand the change, you have run it, and you can answer questions about it. PRs that look like unreviewed agent output will be closed. Write PR descriptions and replies yourself.

## Sending changes

- Run `mise run check` and test the affected workflow by hand in `mise run dev`.
- Bug fixes come with a regression test that fails without the fix.
- Attach screenshots or a recording for visual changes.

## What happens to your PR

Opening a PR does not guarantee a merge. A maintainer may merge it as is, narrow or reshape it, or use it as a reference implementation for a change of their own. When your work shapes what lands, you are credited. A closed PR does not mean the problem was dismissed.

## Licensing

By submitting a contribution you grant us a perpetual, irrevocable, worldwide, royalty-free copyright and patent license to use, modify, sublicense and distribute it under any terms, including proprietary licenses and hosted services. You also confirm you have the right to submit it.

