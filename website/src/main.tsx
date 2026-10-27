import { ViteReactSSG } from 'vite-react-ssg';
import { routes } from './routes';
import './index.css';

// SSG entry point. vite-react-ssg walks `routes` and pre-renders each one to a
// static .html file during `npm run build`. In the browser it hydrates the
// same route tree, so navigation after first load is client-side.
export const createRoot = ViteReactSSG({ routes });
