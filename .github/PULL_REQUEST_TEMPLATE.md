## Summary

<!-- What & why; link the spec/plan section this implements -->

## Platform test checklist (master plan §5)

- [ ] macOS (manual or CI matrix)
- [ ] Windows (manual or CI matrix)
- [ ] n/a — docs/CI-only change

## Gates

- [ ] `quick` green (fmt · clippy -D warnings · tests · license gate · SPDX)
- [ ] `matrix` green (macOS + Windows build & bundle)
- [ ] Security-sensitive paths touched? → security-auditor APPROVE linked
