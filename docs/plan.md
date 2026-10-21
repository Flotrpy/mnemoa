# mnemoa build plan

Progress memory for this repo. Tick an item in the same commit that completes it.
A fresh session resumes from this file plus `CONTEXT.md`.

Branch: `claude/tender-planck-m6x8b5`. Every commit must pass
`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`
and `cargo test --workspace`.

- [x] 1. chore: init cargo workspace with core, cli, api, bench crates
- [x] 2. docs: add minimal CONTEXT.md, AGENTS.md and CLAUDE.md (@AGENTS.md)
- [x] 3. docs: add verified research notes
- [x] 4. ci: add GitHub Actions for fmt, clippy and tests on linux and macos
- [x] 5. feat(core): discover agent context surfaces with per-target loading model
- [x] 6. feat(core): markdown document model with source offsets, rules and front matter
- [x] 7. feat(core): token estimation and always-loaded budget per target
- [x] 8. feat(core): diagnostics, rule registry and .mnemoa.toml config
- [ ] 9. feat(core): MNA006 blind-reference and MNA007 broken-reference
- [ ] 10. feat(core): MNA008 stale-command across Makefile, justfile, package.json, cargo and pyproject
- [ ] 11. feat(core): MNA001 context-bloat and MNA009 discoverable-content
- [ ] 12. feat(core): MNA002 lint-leakage with enforcer detection catalog
- [ ] 13. feat(core): MNA003 skill-leakage and MNA004 conflicting-instructions
- [ ] 14. feat(core): git history trait with gix backend; MNA005 init-fossilization
- [ ] 15. feat(core): MNA010 cross-file-divergence, MNA013 unloaded-file and rationale parser (MNA011)
- [ ] 16. feat(cli): mnemoa scan with pretty, json, sarif and github formats plus baseline
- [ ] 17. ci: dogfood mnemoa scan on this repo
- [ ] 18. feat(core): reference drift and diff-aware --base mode
- [ ] 19. feat(core): plan drift for Spec Kit, Kiro tasks and Cline memory bank
- [ ] 20. feat(cli): mnemoa drift and mnemoa budget
- [ ] 21. feat(core): promotion catalog, artifact emitters and ledger; MNA012
- [ ] 22. feat(core): optional LLM drafter for ast-grep rules with sample validation
- [ ] 23. feat(cli): mnemoa promote (dry-run diff, --apply) and mnemoa retire
- [ ] 24. feat(core,cli): mnemoa forget prune plan and mnemoa init scaffolding
- [ ] 25. feat(api): axum server with scan, budget, drift, promote, forget, rules, health and stats
- [ ] 26. feat: Dockerfile, docker-compose with optional Ollama, .env.example and mnemoa.example.toml
- [ ] 27. feat(sdk): python sdk with sync and async clients, tests and examples/basic_usage.py
- [ ] 28. feat(bench): mnemoa-bench with 12 suites, --filter and --json
- [ ] 29. test: fixture corpus, insta snapshots, proptest parser fuzzing and coverage target
- [ ] 30. feat(notebooks): Colab notebook to build, run corpus study and chart benchmarks
- [ ] 31. docs: architecture, api and rules docs; CONTRIBUTING; action.yml
- [ ] 32. docs: README in mnemo format with real numbers and TBDs

If an item splits, renumber this list and keep going.
