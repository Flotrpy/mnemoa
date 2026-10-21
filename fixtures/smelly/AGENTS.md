# Smelly project

<!-- why: reviewers kept asking | retire-when: check:indentation -->
- Use 2 spaces for indentation.
- [API docs](docs/api.md)
- Read [setup](docs/setup.md) before onboarding.
- Run `make check` before committing.
- Install with `npm install`.
- Use pnpm for everything.

## Stack

- TypeScript
- React
- Vitest

```
.
├── src/
│   └── index.ts
├── docs/
└── package.json
```

## Adding a new endpoint

1. Add the route in `src/routes.ts`.
2. Add the handler next to it.
3. Add a test.
4. Update the API reference.
