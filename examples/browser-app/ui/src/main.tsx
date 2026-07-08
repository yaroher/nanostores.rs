import { useStore } from "@nanostores/preact";
import { listenKeys } from "nanostores";
import { render } from "preact";
import { useEffect, useState } from "preact/hooks";
import { increment, load_user, type User } from "./pkg/browser_app_core";
import { createStores, type AppStores } from "./pkg/browser_app_core_stores";
import "./styles.css";

const root = document.getElementById("app");
if (!root) {
  throw new Error("missing #app root");
}

render(<App stores={await createStores()} />, root);

function App({ stores }: { stores: AppStores }) {
  const count = useStore(stores.count);
  const doubled = useStore(stores.doubled);
  const summary = useStore(stores.summary);
  const user = useStore(stores.user);
  const [countInput, setCountInput] = useState(String(count));
  const [draftUser, setDraftUser] = useState<User>(user);
  const [events, setEvents] = useState<string[]>([]);

  useEffect(() => {
    setCountInput(String(count));
  }, [count]);

  useEffect(() => {
    setDraftUser(user);
  }, [user]);

  useEffect(() => {
    return listenKeys(stores.user, ["name", "age", "displayName"], (nextUser, changedKey) => {
      setEvents((current) =>
        [`${changedKey ?? "all"} -> ${formatUser(nextUser)}`, ...current].slice(0, 6),
      );
    });
  }, [stores.user]);

  return (
    <main class="app-shell">
      <section class="summary-band">
        <Metric label="count" value={String(count)} />
        <Metric label="computed" value={String(doubled)} />
        <Metric label="batched" value={summary} wide />
      </section>

      <section class="control-grid">
        <form
          class="panel"
          onSubmit={(event) => {
            event.preventDefault();
            stores.count.set(Number(countInput));
          }}
        >
          <label htmlFor="count-input">Count</label>
          <div class="inline-controls">
            <input
              id="count-input"
              type="number"
              value={countInput}
              onInput={(event) => setCountInput(event.currentTarget.value)}
            />
            <button type="submit">Set</button>
            <button type="button" onClick={() => increment()}>
              Increment
            </button>
            <button type="button" onClick={() => void load_user("Grace")}>
              Load user (async)
            </button>
          </div>
        </form>

        <form
          class="panel"
          onSubmit={(event) => {
            event.preventDefault();
            stores.user.setKey("name", draftUser.name);
            stores.user.setKey("age", Number(draftUser.age));
            stores.user.setKey("displayName", draftUser.displayName || undefined);
          }}
        >
          <label htmlFor="name-input">Name</label>
          <input
            id="name-input"
            type="text"
            value={draftUser.name}
            onInput={(event) => setDraftUser({ ...draftUser, name: event.currentTarget.value })}
          />

          <label htmlFor="age-input">Age</label>
          <input
            id="age-input"
            type="number"
            min="0"
            value={draftUser.age}
            onInput={(event) =>
              setDraftUser({ ...draftUser, age: Number(event.currentTarget.value) })
            }
          />

          <label htmlFor="display-name-input">Display name</label>
          <input
            id="display-name-input"
            type="text"
            value={draftUser.displayName ?? ""}
            onInput={(event) =>
              setDraftUser({ ...draftUser, displayName: event.currentTarget.value || undefined })
            }
          />

          <button type="submit">Update user</button>
        </form>

        <section class="panel event-panel">
          <div class="event-heading">listenKeys</div>
          <ol>
            {events.map((event, index) => (
              <li key={`${event}-${index}`}>{event}</li>
            ))}
          </ol>
        </section>
      </section>
    </main>
  );
}

function Metric({ label, value, wide = false }: { label: string; value: string; wide?: boolean }) {
  return (
    <div class={wide ? "metric wide" : "metric"}>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function formatUser(user: User): string {
  const displayName = user.displayName ? `, ${user.displayName}` : "";
  return `${user.name}, ${user.age}${displayName}`;
}
