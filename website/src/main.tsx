import { ViteReactSSG } from 'vite-react-ssg';
import { routes } from './routes';
import './index.css';

// SSG entry point. vite-react-ssg walks `routes` and pre-renders each one to a
// static .html file during `npm run build`. In the browser it hydrates the
// same route tree, so navigation after first load is client-side.
//
// `basename` MUST be passed so the client-side router knows the public base
// path. Vite exposes it as import.meta.env.BASE_URL (derived from `base` in
// vite.config.ts, which the CI workflow sets to "/clock-bound/" for the
// GitHub Pages project site). Without it the router defaults to "/", so under
// the /clock-bound/ subpath the browser URL matches no route and falls through
// to the 404 page after hydration. See vite-react-ssg README, "Public Base
// Path".
export const createRoot = ViteReactSSG({
  basename: import.meta.env.BASE_URL,
  routes,
});
