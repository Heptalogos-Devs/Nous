// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { z } from "zod";

// Native input constraints are published unchanged to the Configuration Catalog.
const endpointInput = z
  .string()
  .url()
  .regex(
    /^(?:https:\/\/[^\s/@?#]+|http:\/\/(?:127\.0\.0\.1|\[::1\])(?::\d+)?)(?:\/[^\s?#]*)?$/,
    "Endpoint requires credential-free HTTPS or literal loopback HTTP",
  );
export const remoteEndpointSchema = endpointInput
  .transform((value) => new URL(value).toString().replace(/\/$/, ""))
  .pipe(endpointInput);
