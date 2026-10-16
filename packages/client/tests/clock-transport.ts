// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { createGrpcTransport } from "@connectrpc/connect-node";
import { createNousClient } from "../src/index.js";
import { mcpConnection } from "../../../apps/nous-cli/src/mcp-scope.js";
import type { connectNousInstance } from "../src/node.js";
const endpoint = process.argv[2]!;
const connect = (token: string) =>
  createNousClient(
    createGrpcTransport({
      baseUrl: endpoint,
      interceptors: [
        (next) => async (request) => {
          request.header.set("authorization", `Bearer ${token}`);
          return next(request);
        },
      ],
    }),
  );
const client = connect("clock-fixture");
const subject = await client.subjects.createSubject({
  operationId: randomUUID(),
  capabilities: { memory: true },
});
const subjectId = subject.subjectId;
await assert.rejects(connect("wrong").clocks.readClock({ subjectId }));
const configured = await client.clocks.configureClock({
  subjectId,
  operationId: randomUUID(),
  timeDomainRef: "time:transport",
  rate: 0,
});
assert.equal(configured.timeDomainRef, "time:transport");
const operationId = randomUUID();
const advanced = await client.clocks.advanceClock({
  subjectId,
  operationId,
  seconds: 10,
});
const replay = await client.clocks.advanceClock({
  subjectId,
  operationId,
  seconds: 10,
});
assert.deepEqual(advanced, replay);
assert.equal(
  (
    await client.clocks.resumeClock({
      subjectId,
      operationId: randomUUID(),
      rate: 60,
    })
  ).rate,
  60,
);
assert.equal(
  (await client.clocks.pauseClock({ subjectId, operationId: randomUUID() }))
    .rate,
  0,
);
assert.equal((await client.clocks.readClock({ subjectId })).rate, 0);
const observed = await client.clocks.readClock({ subjectId });
const attempts = [
  { subjectId, operationId: randomUUID(), expectedRevision: observed.revision },
  { subjectId, operationId: randomUUID(), expectedRevision: observed.revision },
];
const raced = await Promise.allSettled([
  client.clocks.pauseClock(attempts[0]!),
  client.clocks.resumeClock({ ...attempts[1]!, rate: 5 }),
]);
assert.equal(raced.filter((result) => result.status === "fulfilled").length, 1);
assert.equal(raced.filter((result) => result.status === "rejected").length, 1);
const winnerIndex = raced[0]!.status === "fulfilled" ? 0 : 1;
const winner = raced[winnerIndex]!;
assert.equal(winner.status, "fulfilled");
const changed = await client.clocks.configureClock({
  subjectId,
  operationId: randomUUID(),
  timeDomainRef: "time:transport",
  rate: 7,
});
const replayWinner = () =>
  winnerIndex === 0
    ? client.clocks.pauseClock(attempts[0]!)
    : client.clocks.resumeClock({ ...attempts[1]!, rate: 5 });
assert.deepEqual(await replayWinner(), winner.value);
const current = await client.clocks.readClock({ subjectId });
assert.equal(current.revision, changed.revision);
assert.equal(current.rate, 7);
await assert.rejects(() =>
  client.clocks.pauseClock({
    subjectId,
    operationId: randomUUID(),
    expectedRevision: observed.revision,
  }),
);
const connection = { ...client } as Awaited<
  ReturnType<typeof connectNousInstance>
>;
const native = mcpConnection(connection, false);
assert.throws(
  () =>
    native.clocks.advanceClock({
      subjectId,
      operationId: randomUUID(),
      seconds: 1,
    }),
  /does not grant/,
);
console.log(
  "clock authenticated public SDK and native management boundary PASS",
);
