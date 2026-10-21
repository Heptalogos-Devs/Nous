// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import {Decimal} from 'decimal.js';
import type {LanguageModelV4Usage} from '@ai-sdk/provider';
import {MoneySchema,type Money} from '@nous-wave/contracts/common';
import {NormalizedUsageSchema,type NormalizedUsage,type PriceBook,type TokenEstimate} from '@nous-wave/contracts/models';
const Exact=Decimal.clone({precision:1024});
export function languageUsage(value:LanguageModelV4Usage):NormalizedUsage {
 return NormalizedUsageSchema.parse({inputTokens:value.inputTokens.total,outputTokens:value.outputTokens.total,cacheReadTokens:value.inputTokens.cacheRead,cacheWriteTokens:value.inputTokens.cacheWrite,reasoningTokens:value.outputTokens.reasoning});
}
/** Native uncached tokens and V4 total tokens have different meanings. Normalize once at that boundary. */
export function nativeLanguageUsage(value:{input?:number;output?:number;cacheRead?:number;cacheWrite?:number;reasoning?:number}):NormalizedUsage {
 const input=value.input===undefined?undefined:value.input+(value.cacheRead??0)+(value.cacheWrite??0);
 return NormalizedUsageSchema.parse({inputTokens:input,outputTokens:value.output,cacheReadTokens:value.cacheRead,cacheWriteTokens:value.cacheWrite,reasoningTokens:value.reasoning});
}
export function modelCost(book:PriceBook|undefined,usage:NormalizedUsage|undefined):Money|null {
 if(!book||!usage)return null;
 const quantities=new Map<string,string>();
 if(usage.inputTokens!==undefined){
  quantities.set('input_tokens',String(usage.inputTokens));
  if(usage.cacheReadTokens!==undefined&&usage.cacheWriteTokens!==undefined)quantities.set('uncached_input_tokens',String(Math.max(0,usage.inputTokens-usage.cacheReadTokens-usage.cacheWriteTokens)));
 }
 if(usage.outputTokens!==undefined)quantities.set('output_tokens',String(usage.outputTokens));
 if(usage.cacheReadTokens!==undefined)quantities.set('cache_read_tokens',String(usage.cacheReadTokens));
 if(usage.cacheWriteTokens!==undefined)quantities.set('cache_write_tokens',String(usage.cacheWriteTokens));
 for(const quantity of usage.units??[])if(quantity.source!=='estimated')quantities.set(quantity.unit,quantity.quantity);
 let amount=new Exact(0);
 for(const rate of book.rates){const quantity=quantities.get(rate.unit);if(quantity===undefined)return null;amount=amount.plus(new Exact(rate.amount.amount).mul(quantity).div(rate.per));}
 return MoneySchema.parse({amount:amount.toFixed(),currency:book.rates[0].amount.currency});
}
export function reserveCost(book:PriceBook|undefined,estimate:TokenEstimate,maxOutputTokens:number):Money|null {
 if(!book)return null;
 // A generic token estimate cannot reserve duration/character/image prices.
 if(book.rates.some(rate=>!['input_tokens','output_tokens','uncached_input_tokens','cache_read_tokens','cache_write_tokens'].includes(rate.unit)))return null;
 const inputRates=book.rates.filter(rate=>rate.unit!=='output_tokens');
 const input=inputRates.reduce((max,rate)=>Decimal.max(max,new Exact(rate.amount.amount).div(rate.per)),new Exact(0));
 const output=book.rates.filter(rate=>rate.unit==='output_tokens').reduce((sum,rate)=>sum.plus(new Exact(rate.amount.amount).div(rate.per)),new Exact(0));
 return MoneySchema.parse({amount:input.mul(estimate.value).plus(output.mul(maxOutputTokens)).toFixed(),currency:book.rates[0].amount.currency});
}
