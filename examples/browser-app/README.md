# Browser app example

This example demonstrates Rust-owned nanostores projected into real JavaScript
nanostores stores.

```bash
npm install
make build-example
npm run dev --workspace browser-app-ui
```

The UI consumes `count`, `user`, Rust `computed` (`doubled`), and Rust
`batched` (`summary`) stores through the generated
`ui/src/pkg/browser_app_core_stores.ts` wrapper, then reads them with
`@nanostores/preact` `useStore`.
