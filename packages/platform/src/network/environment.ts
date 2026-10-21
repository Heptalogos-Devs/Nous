// Copied/adapted from Undici 8.10.0, commit c8d80e6b2dcfab282557b08f51352937bc9e5692,
// lib/dispatcher/env-http-proxy-agent.js. Copyright Node.js contributors; MIT.
// Route parsing stays identical to the native agent; only the selected URI is returned.
export interface EnvironmentProxySnapshot {
  httpProxy: string;
  httpsProxy: string;
  noProxy: string;
}
export function environmentProxySnapshot(
  noProxy?: string,
): EnvironmentProxySnapshot {
  return {
    httpProxy: process.env.http_proxy ?? process.env.HTTP_PROXY ?? '',
    httpsProxy: process.env.https_proxy ?? process.env.HTTPS_PROXY ?? '',
    noProxy: noProxy ?? process.env.no_proxy ?? process.env.NO_PROXY ?? '',
  };
}
const DEFAULT_PORTS: Record<string, number> = { 'http:': 80, 'https:': 443 };
export class EnvironmentProxyRoute {
  #noProxyValue: string | null = null;
  #noProxyEntries: { hostname: string; port: number }[] = [];
  #opts: EnvironmentProxySnapshot;
  constructor(options: EnvironmentProxySnapshot) {
    this.#opts = options;
    this.#parseNoProxy();
  }
  select(url: URL): string | undefined {
    let { protocol, host: hostname, port: portText } = url;

    // Remove the port suffix (e.g. ":8080") and then strip surrounding
    // brackets from IPv6 literals (e.g. "[::1]" -> "::1") so that the
    // result matches the unbracketed form stored by #parseNoProxy.
    hostname = hostname
      .replace(/:\d*$/, '')
      .replace(/^\[(.+)\]$/, '$1')
      .toLowerCase();
    const port = Number.parseInt(portText, 10) || DEFAULT_PORTS[protocol] || 0;
    if (!this.#shouldProxy(hostname, port)) {
      return undefined;
    }
    if (protocol === 'https:') {
      return this.#opts.httpsProxy || this.#opts.httpProxy || undefined;
    }
    return this.#opts.httpProxy || undefined;
  }

  #shouldProxy(hostname: string, port: number) {
    if (this.#noProxyChanged) {
      this.#parseNoProxy();
    }

    if (this.#noProxyEntries.length === 0) {
      return true; // Always proxy if NO_PROXY is not set or empty.
    }
    if (this.#noProxyValue === '*') {
      return false; // Never proxy if wildcard is set.
    }

    for (let i = 0; i < this.#noProxyEntries.length; i++) {
      const entry = this.#noProxyEntries[i];
      if (entry.port && entry.port !== port) {
        continue; // Skip if ports don't match.
      }
      // Don't proxy if the hostname is equal with the no_proxy host.
      if (hostname === entry.hostname) {
        return false;
      }
      // Don't proxy if the hostname is the subdomain of the no_proxy host.
      // Reference - https://github.com/denoland/deno/blob/6fbce91e40cc07fc6da74068e5cc56fdd40f7b4c/ext/fetch/proxy.rs#L485
      if (
        hostname.slice(-(entry.hostname.length + 1)) === `.${entry.hostname}`
      ) {
        return false;
      }
    }

    return true;
  }

  #parseNoProxy() {
    const noProxyValue = this.#opts.noProxy ?? this.#noProxyEnv;
    const noProxySplit = noProxyValue.split(/[,\s]/);
    const noProxyEntries: { hostname: string; port: number }[] = [];

    for (let i = 0; i < noProxySplit.length; i++) {
      const entry = noProxySplit[i];
      if (!entry) {
        continue;
      }

      // An IPv6 entry with a port must be bracketed: [::1]:443.
      // A bare IPv6 address like ::1 contains colons that must not be
      // confused with a host:port separator, so we handle it separately.
      let hostname, port;
      const ipv6WithPort = entry.match(/^\[(.+)\]:(\d+)$/);
      if (ipv6WithPort) {
        hostname = ipv6WithPort[1];
        port = Number.parseInt(ipv6WithPort[2], 10);
      } else {
        // Bracketed IPv6 without port, or plain hostname[:port], or bare IPv6.
        // Strip optional brackets first.
        const unbracketed = entry.replace(/^\[(.+)\]$/, '$1');
        // A bare IPv6 address contains multiple colons; a hostname:port entry
        // has exactly one colon followed by digits. Only attempt host:port
        // splitting when that is unambiguously the case.
        const colonCount = (unbracketed.match(/:/g) || []).length;
        const parsed = colonCount === 1 && unbracketed.match(/^(.+):(\d+)$/);
        hostname = parsed ? parsed[1] : unbracketed;
        port = parsed ? Number.parseInt(parsed[2], 10) : 0;
      }

      noProxyEntries.push({
        // strip leading dot or asterisk with dot
        hostname: hostname.replace(/^\*?\./, '').toLowerCase(),
        port,
      });
    }

    this.#noProxyValue = noProxyValue;
    this.#noProxyEntries = noProxyEntries;
  }

  get #noProxyChanged() {
    if (this.#opts.noProxy !== undefined) {
      return false;
    }
    return this.#noProxyValue !== this.#noProxyEnv;
  }

  get #noProxyEnv() {
    return process.env.no_proxy ?? process.env.NO_PROXY ?? '';
  }
}
