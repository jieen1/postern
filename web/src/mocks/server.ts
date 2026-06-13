import { setupServer } from 'msw/node';
import { handlers } from './handlers';

/** Node MSW server — used by Vitest (see src/test/setup.ts). */
export const server = setupServer(...handlers);
