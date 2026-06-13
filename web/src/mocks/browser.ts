import { setupWorker } from 'msw/browser';
import { handlers } from './handlers';

/** Browser MSW worker — started in dev (and when VITE_ENABLE_MSW !== false). */
export const worker = setupWorker(...handlers);
