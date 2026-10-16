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
