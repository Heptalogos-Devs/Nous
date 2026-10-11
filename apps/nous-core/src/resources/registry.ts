// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { ExternalResourceAdapter, ResourceBinding } from "./adapter.js";

interface HostedResourceProvider {
  adapterKind: string;
  providerProfile: string;
  adapter: ExternalResourceAdapter;
}
const key = (kind: string, profile: string) => JSON.stringify([kind, profile]);

export class ResourceRegistry {
  private readonly adapters = new Map<string, ExternalResourceAdapter>();
  constructor(providers: readonly HostedResourceProvider[] = []) {
    for (const provider of providers) {
      const identity = key(provider.adapterKind, provider.providerProfile);
      if (this.adapters.has(identity))
        throw new Error("Duplicate hosted Resource provider identity");
      this.adapters.set(identity, provider.adapter);
    }
  }
  resolve(binding: ResourceBinding): ExternalResourceAdapter | undefined {
    return this.adapters.get(key(binding.adapterKind, binding.providerProfile));
  }
}
