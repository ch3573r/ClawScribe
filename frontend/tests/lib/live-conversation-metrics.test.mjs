import assert from "node:assert/strict";
import test from "node:test";
import { createHookHarness, deferred, flush } from "./hook-harness.mjs";
import { loadTsModule } from "./load-ts-module.mjs";

const jsx = (type, props) => ({ type, props });
const nodes = root => Array.isArray(root) ? root.flatMap(nodes) : root?.props
  ? [root, ...nodes(root.props.children)] : [];
const copy = root => Array.isArray(root) ? root.map(copy).join("") : root?.props
  ? copy(root.props.children) : typeof root === "string" ? root : "";

function harness(invoke) {
  const hooks = createHookHarness();
  let receive;
  let unlistened = false;
  const { ConversationReadout } = loadTsModule("src/components/LiveConversationMetrics.tsx", {
    react: hooks.react,
    "react/jsx-runtime": { jsx, jsxs: jsx },
    "next/link": { default: "Link", __esModule: true },
    "@tauri-apps/api/core": { invoke },
    "@tauri-apps/api/event": {
      listen: async (name, onEvent) => {
        assert.equal(name, "live-conversation-metrics");
        receive = payload => onEvent({ payload });
        return () => { unlistened = true; };
      },
    },
    "@/components/ui/button": { Button: "Button" },
    "@/components/ui/page-section": { PageSection: "PageSection" },
    "@/components/ui/progress": { Progress: "Progress" },
  });

  return {
    render: () => hooks.render(() => ConversationReadout({ sessionId: "current" })),
    event: payload => receive(payload),
    unmount: () => hooks.unmount(),
    unlistened: () => unlistened,
  };
}

const snapshot = (availability, revision, sides = null) => ({
  session_id: "current", revision, availability, sides,
  totals: { speech_seconds: 62, words: 72, questions: 2 },
});

test("live conversation shows loading, empty, and total-only states", async () => {
  const pending = deferred();
  const metrics = harness(() => pending.promise);
  assert.match(copy(metrics.render()), /Reading conversation metrics/);
  pending.resolve(snapshot("waiting_for_speech", 0));
  await flush();
  assert.match(copy(metrics.render()), /Waiting for finalized speech/);

  metrics.event(snapshot("labels_off", 1));
  const off = metrics.render();
  assert.match(copy(off), /Me and Participants labels are off/);
  assert.match(copy(off), /1:02/);
  assert.equal(nodes(off).find(node => node.type === "Link").props.href, "/settings?tab=transcription");

  metrics.event(snapshot("unclear_source", 2));
  const unclear = metrics.render();
  assert.match(copy(unclear), /could not be separated by source/);
  assert.equal(nodes(unclear).some(node => node.type === "Link" || node.type === "Progress"), false);
  metrics.unmount();
  await flush();
  assert.equal(metrics.unlistened(), true);
});

test("live conversation formats the ratio, pace and stretches and ignores stale snapshots", async () => {
  const pending = deferred();
  const metrics = harness(() => pending.promise);
  metrics.render();
  metrics.event({
    ...snapshot("available", 2, {
      me_share: 2 / 3, unclear_share: 0, pace_wpm: null, recent_pace_wpm: 127,
      me_questions: 1, others_questions: 1,
      current_monologue_seconds: 9.7, longest_monologue_seconds: 62,
    }),
  });
  metrics.event({ ...snapshot("labels_off", 99), session_id: "previous" });
  pending.resolve(snapshot("waiting_for_speech", 0));
  await flush();

  const view = metrics.render();
  const ratio = nodes(view).find(node => node.type === "Progress");
  assert.equal(ratio.props["aria-label"], "Talk time");
  assert.equal(ratio.props.getValueLabel(), "You 67%, others 33%");
  assert.match(copy(view), /127 wpm/);
  assert.match(copy(view), /Unavailable/);
  assert.match(copy(view), /0:09/);
  assert.match(copy(view), /1:02/);
  metrics.unmount();
});
