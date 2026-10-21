import { generateOpenApiClient } from '@nous-wave/platform/devtools/openapi';
import { resolve } from 'node:path';
import { createHttp } from './http.js';

const server = await createHttp();
try {
  await server.ready();
  await generateOpenApiClient({
    document: server.swagger(),
    schemaPath: resolve('packages/contracts/openapi/nous.json'),
    clientPath: resolve('packages/client/src/generated'),
    temporaryRoot: resolve('../temp/generated'),
  });
  process.stdout.write('Generated Nous Client with OpenAPI Generator 7.26.0\n');
} finally {
  await server.close();
}
