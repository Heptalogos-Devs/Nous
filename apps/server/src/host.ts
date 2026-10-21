// Adapted from Heptalogos apps/server/main.ts. Product composition only.
import { createBootstrap,type BootstrapOptions } from '@nous-wave/platform/bootstrap';
import { withNous } from '@nous-wave/nous';
import { createHttp } from './http.js';

export async function runHost(options:Omit<BootstrapOptions,'product'>) {
  const view=await createBootstrap({...options,product:'nous'}).inspect();
  await withNous(view,async app=>{
    const stopped=Promise.withResolvers<void>();
    let closing:Promise<void>|undefined;
    const stop=(mode:'graceful'|'force'='graceful')=>closing??=(async()=>{
      await app.host.drain();
      try {await http.close();await app.host.stop(mode);stopped.resolve();}
      catch(error){stopped.reject(error);throw error;}
    })();
    const http=await createHttp(app,{baseURL:view.bootstrap.listen.publicBaseURL,stop});
    const interrupt=()=>{void stop().catch(()=>{});};
    process.once('SIGINT',interrupt);process.once('SIGTERM',interrupt);
    try {
      await app.host.start();
      await http.listen({host:view.bootstrap.listen.host,port:view.bootstrap.listen.port});
      app.telemetry.logger.info({event:'host.started'});
      await stopped.promise;
    } finally {
      process.removeListener('SIGINT',interrupt);process.removeListener('SIGTERM',interrupt);
      await stop();
    }
  });
}
