import { createNous } from '@nous-wave/nous';
import { loadBootstrap } from '@nous-wave/nous';
import { createHttp } from './http.js';
const config=await loadBootstrap(process.argv.find(arg=>arg.startsWith('--config='))?.slice(9));
const app=await createNous(config);
const http=await createHttp(app,{baseURL:`http://${config.host}:${config.port}`});
await http.listen({host:config.host,port:config.port});
for(const signal of ['SIGTERM','SIGINT'])process.once(signal,()=>{void (async()=>{await http.close();await app.close();process.exit(0);})();});
