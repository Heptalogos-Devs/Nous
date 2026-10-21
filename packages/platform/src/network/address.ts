import { lookup as dnsLookup, type LookupOptions } from 'node:dns';
import type { LookupFunction } from 'node:net';
import ipaddr from 'ipaddr.js';
import { problem } from '../errors.js';
import type { ResourceNetworkScope } from './contracts.js';
export function allowedAddress(
  resource: ResourceNetworkScope,
  value: string,
): boolean {
  let address: ipaddr.IPv4 | ipaddr.IPv6;
  try {
    address = ipaddr.process(value.replace(/^\[|\]$/g, ''));
  } catch {
    return false;
  }
  if (address.range() === 'unicast') return true;
  if (resource.addressScope !== 'configured_private') return false;
  return (resource.allowedPrivateRanges ?? []).some((range) => {
    try {
      const [base, prefix] = ipaddr.parseCIDR(range);
      return (
        address.kind() === base.kind() && address.match(base as never, prefix)
      );
    } catch {
      throw problem(
        'network.invalid_address_range',
        'validation',
        '网络地址范围配置无效',
      );
    }
  });
}
export function assertLiteralAddress(
  resource: ResourceNetworkScope,
  hostname: string,
) {
  const value = hostname.replace(/^\[|\]$/g, '');
  if (ipaddr.isValid(value) && !allowedAddress(resource, value))
    throw problem(
      'network.unauthorized_target',
      'forbidden',
      '网络地址超出资源范围',
    );
}
/** Filter the addresses returned to the actual Node connector, not a preflight DNS guess. */
export function guardedLookup(resource: ResourceNetworkScope): LookupFunction {
  return (hostname, options, callback) => {
    dnsLookup(
      hostname,
      { ...options, all: true, verbatim: true },
      (error, addresses) => {
        if (error) {
          callback(error, '', 0);
          return;
        }
        const allowed = addresses.filter((value) =>
          allowedAddress(resource, value.address),
        );
        if (!allowed.length) {
          callback(
            problem(
              'network.unauthorized_target',
              'forbidden',
              'DNS目标超出资源地址范围',
            ),
            '',
            0,
          );
          return;
        }
        if ((options as LookupOptions).all)
          (
            callback as unknown as (
              error: Error | null,
              addresses: typeof allowed,
            ) => void
          )(null, allowed);
        else callback(null, allowed[0].address, allowed[0].family);
      },
    );
  };
}
