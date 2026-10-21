// Copied from Vercel AI SDK 7.0.146 src/embed/embed-many.ts
// commit 0a5c786ef65bcd835328fb543685f6d8ef233dae. Apache-2.0; see NOTICE.
// This freezes identities; the SDK still owns execution and parallelism.
export function embeddingPlan(values:readonly string[],count:number,bytes:number):string[][]{
 if(count<=0||bytes<=0)throw new Error('Embedding batch limits must be greater than 0');
 const chunks:string[][]=[];let current:string[]=[];let currentBytes=0;
 for(const value of values){const inputBytes=new TextEncoder().encode(value).length;
  if(current.length>0&&(current.length>=count||currentBytes+inputBytes>bytes)){chunks.push(current);current=[];currentBytes=0;}
  current.push(value);currentBytes+=inputBytes;
 }
 if(current.length)chunks.push(current);return chunks;
}
