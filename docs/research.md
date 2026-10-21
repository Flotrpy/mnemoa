# Research behind mnemoa

This file records the evidence mnemoa is built on. Each claim cites its source and carries a status:

- **primary**: checked against the source itself.
- **search**: checked against search-engine extracts of the source page and at least one independent summary, because the source host was unreachable from the build environment.
- **unverified**: seen only in secondary sources, or not found. Do not repeat these as fact.

The build environment's network policy blocked arxiv.org, trychroma.com, developers.openai.com, kiro.dev, docs.cline.bot, docs.github.com, github.blog, agents.md, ollama.com, agentpatterns.ai and developer.upsun.com. Anyone with access should upgrade **search** items to **primary** and fix this file where they differ.

Last checked: 2026-10-21.

## 1. More context can hurt

**Context rot (Chroma, July 2025)**. *search.* Chroma tested 18 LLMs (including GPT-4.1, Claude 4, Gemini 2.5 and Qwen3) on deliberately simple tasks. Reliability dropped as input length grew, even when the task stayed trivial. Distractors that were topically close to the answer made it worse. Source: <https://research.trychroma.com/context-rot>.

**Lost in the middle (Liu et al., TACL 2024)**. *search.* On multi-document QA and key-value retrieval, accuracy was highest when the relevant information sat at the start or end of the input and lowest in the middle, a U-shaped curve, including for long-context models. arXiv: <https://arxiv.org/abs/2307.03172>. TACL 12:157–173, DOI 10.1162/tacl_a_00638.

**Context length alone hurts (Du et al., Findings of EMNLP 2025)**. *search.* Across five models and math, QA and coding tasks, performance fell by 13.9% to 85% as input grew, while staying within each model's claimed window. The drop persisted when the extra tokens were whitespace or were masked, so length itself, not distraction, causes some of the loss. The authors propose having the model recite the evidence before answering. arXiv: <https://arxiv.org/abs/2510.05381>.

**Effective context engineering for AI agents (Anthropic, 29 Sept 2025)**. *primary.* Treats context as a finite resource with diminishing returns and describes an "attention budget". The stated goal is the "smallest possible set of high-signal tokens". Recommends just-in-time retrieval through lightweight identifiers, and notes kept outside the context window. <https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents>.

**Implication for mnemoa:** every always-loaded line costs attention. Measure always-loaded context per agent (`mnemoa budget`) and move content out of it (`mnemoa forget`).

## 2. AGENTS.md and CLAUDE.md specifically

**Evaluating AGENTS.md (Gloaguen, Mündler, Müller, Raychev, Vechev; ICLR 2026)**. *search.* Four agents on SWE-bench Lite and a new 138-task benchmark from niche repositories, comparing no context file, an LLM-generated file and a developer-written file. Agents did follow the files: they tested and explored more. LLM-generated files lowered success by about 3% on average and raised inference cost by over 20%. Developer-written files raised success by about 4% on average but also raised cost (up to about 19%). The authors conclude that unnecessary requirements make tasks harder and that human-written files should state only minimal requirements. arXiv: <https://arxiv.org/abs/2602.11988>. Summary: <https://developer.upsun.com/posts/ai/agents-md-less-is-more>.
*Correction to the original brief:* the brief said context files "tended to reduce task success" overall; the effect depends on who wrote the file. A "+2.7% when documentation was stripped" figure could not be found in any source and is **unverified**; mnemoa does not cite it.

**Configuration smells in AGENTS.md files (SCAM 2026)**. *search, plus paper excerpts in search results.* A six-smell catalog built from grey literature and pull requests, measured on the 100 most-starred repositories with a root AGENTS.md or CLAUDE.md. 91 of 100 had at least one smell. Prevalence: lint leakage 62%, context bloat 42%, skill leakage 35%, conflicting instructions 28%, init fossilization 24%, blind references 16%. Their LLM-based detectors reached 93% precision on lint leakage and only 57% on conflicting instructions. The 82% (skill leakage) and 87% (blind references) precision figures in the brief are **unverified**. arXiv: <https://arxiv.org/abs/2606.15828>.

**Why does CLAUDE.md keep growing? (Chakrabarti, Aug 2026)**. *search.* Across 247,694 instruction lifetimes in 1,867 repositories, instruction files grew 226% and gained about 4.9 net instructions per commit; older instructions were less likely to be deleted. The argument: adding a rule is cheap, but proving a rule safe to delete needs a search over subsets once its reason is lost. Comments that record the reasoning removed 99.3% of excess instructions in a controlled IFEval-based setting and improved real-world instruction following by up to 23.1% (WildIFEval). Also: 77.3% of deletions come from one commit that rewrites or moves the whole file, and after such a rewrite the file shrinks to 59.5% of its size and regrows to 91.5% within 10 commits. arXiv: <https://arxiv.org/abs/2608.11095>.

**Implication for mnemoa:** a one-off cleanup does not last. Record why each rule exists and when it can go (rationale comments, `mnemoa retire`), and move rules a tool can enforce into the tool (`mnemoa promote`).

## 3. How each agent loads repo memory

**Claude Code** (<https://code.claude.com/docs/en/memory>). *primary.*
- Target under 200 lines per CLAUDE.md; longer files cost context and reduce adherence. Imports organize files but do not reduce cost, because imports load at launch.
- `@path` imports resolve relative to the importing file, recurse to a maximum of four hops, and are ignored inside code spans and fenced blocks.
- CLAUDE.md and CLAUDE.local.md load from the working directory and every directory above it. Files in subdirectories load on demand, when Claude touches a file there.
- Block-level HTML comments in CLAUDE.md are stripped before injection; comments inside code blocks are kept. This makes rationale comments free for Claude Code.
- `.claude/rules/**/*.md` load at launch unless they have a `paths` front-matter list, in which case they load when Claude works with a matching file. `paths` is the only field read.
- Since v2.1.277 Claude Code reads AGENTS.md directly, but by default only when there is no `CLAUDE.md`, `.claude/CLAUDE.md` or `CLAUDE.local.md` at or above the working directory. If one exists, AGENTS.md is not read unless a CLAUDE.md imports it. A `CLAUDE.md` containing `@AGENTS.md` never loads it twice.
- `/doctor prompt-audit` (v2.1.283+) asks the model to find outdated instructions, references to missing files or commands, and contradictions, and proposes edits. This is prior art for mnemoa's scan.

**OpenAI Codex** (<https://developers.openai.com/codex/guides/agents-md>). *search.* In each directory Codex reads `AGENTS.override.md` if present, else `AGENTS.md`, else names in `project_doc_fallback_filenames`; at most one file per directory. Combined project guidance is capped by `project_doc_max_bytes`, 32 KiB by default; files past the cap are not added. Whether HTML comments are stripped is **unverified**.

**AGENTS.md standard** (<https://agents.md/>, repo <https://github.com/agentsmd/agents.md>). *primary (repository).* Plain Markdown with no required fields. Nested files are allowed and the closest file to the edited file wins; explicit chat prompts override it. Stewarded by the Agentic AI Foundation under the Linux Foundation.

**Cursor** (<https://cursor.com/docs/context/rules>). *search.* Project rules are `.mdc` files in `.cursor/rules/` with `description`, `globs` and `alwaysApply` front matter. A plain `.md` file there is ignored. `.cursorrules` is legacy. AGENTS.md is supported as a simple alternative.

**GitHub Copilot**. *unverified (docs.github.com blocked).* mnemoa models the commonly documented layout: `.github/copilot-instructions.md` always applies; `.github/instructions/*.instructions.md` apply by an `applyTo` glob.

**Kiro** (<https://kiro.dev/docs/steering/>). *search.* Steering files in `.kiro/steering/` (workspace) and `~/.kiro/steering/` (global) take an `inclusion` front-matter mode: `always`, `fileMatch` (with `fileMatchPattern`), `manual` or `auto`. AGENTS.md has no inclusion modes and is always included. A user report says `fileMatch` does not fire for global steering files (<https://github.com/kirodotdev/Kiro/issues/6171>). Whether Kiro CLI honors inclusion modes is **unverified**, so mnemoa reports Kiro budgets as a range (always-only to all files).

**Cline**. *primary (docs source in <https://github.com/cline/cline>, `docs/best-practices/memory-bank.mdx`).* The memory bank is `memory-bank/` with `projectbrief.md`, `productContext.md`, `activeContext.md`, `systemPatterns.md`, `techContext.md` and `progress.md`. `activeContext.md` changes most often and should be updated each session. The memory-bank instructions themselves live in a Cline rules file such as `.clinerules/memory-bank.md`.

**GitHub Spec Kit** (<https://github.com/github/spec-kit>). *primary (template).* Features live under `specs/[###-feature-name]/`. `tasks.md` lines follow `- [ ] T012 [P] [US1] Description with file path`, where `[P]` means parallelizable and `[USn]` names the user story. The project memory directory is `.specify/memory/`; the exact constitution filename is **unverified** from the README alone.

**GitHub, "How to write a great agents.md"** (<https://github.blog/ai-and-ml/github-copilot/how-to-write-a-great-agents-md-lessons-from-over-2500-repositories/>). *search.* About Copilot custom agents, from over 2,500 repositories. Advice: be specific (real commands with flags), show a code example instead of describing style, state boundaries such as never committing secrets. It lists project structure among the six core areas, which is in tension with the evidence above; mnemoa therefore reports duplicated structure only at `info` level.

## 4. Prior art

Detection of agent-config problems is crowded. mnemoa does not try to replace these tools.

| Tool | What it does | Status |
|---|---|---|
| [agnix](https://github.com/agent-sh/agnix) | Rust linter and LSP for agent config across many tools: 702 rules, safe/unsafe autofix, MCP server, GitHub Action, editor plugins. | primary |
| [agentslint](https://github.com/toshi0607/agentslint) | TypeScript CI linter: broken file refs, stale commands, token budget (default 4,000), skill front matter, settings schema, secrets. SARIF output. Under-matches on purpose and stays silent when unsure. | primary |
| [katalint](https://pypi.org/project/katalint/) | Python deterministic linter for agent instructions (v0.1.1). Alignment with the SCAM smells is **unverified**. | primary (PyPI) |
| Claude Code `/doctor prompt-audit` | Model-driven audit of instruction files with proposed edits. | primary |

Other names from the brief (ctxlint, claude-config-auditor, AgentLint, agent-config-lint, grippydoc, readme-drift, DocGuard, Earthly Lunar, ast-grep skills) were not checked in this pass.

**Where mnemoa differs.** Existing tools report problems, and agnix and prompt-audit also propose fixes. mnemoa adds:
1. *Promotion*: turning an enforceable prose rule into a check (formatter config, ast-grep rule, hook) and deleting the prose, tracked in a ledger.
2. *Lifecycle*: rationale comments with retirement conditions, aimed at the growth pattern Chakrabarti measured.
3. *Plan drift*: Spec Kit, Kiro and Cline memory-bank files checked against the code and git history.
4. *Per-agent loading model*: what each agent actually loads at start, including files an agent never loads.
5. *One forget plan* that combines the above and shows the before/after cost.

Running agnix alongside mnemoa is recommended for exhaustive per-tool schema validation.

## 5. Not verified in this pass

- Ollama tag `qwen2.5-coder:7b` (ollama.com blocked). It is the drafter's default; change `MNEMOA_LLM_MODEL` if the tag is gone.
- Codex HTML-comment handling; Copilot file layout; Kiro CLI inclusion behavior.
- SCAM precision figures for skill leakage and blind references.
- The Gloaguen "+2.7%" figure.
