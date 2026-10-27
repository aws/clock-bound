import type { RouteRecord } from 'vite-react-ssg';
import LandingPage from './pages/LandingPage';
import NotFound from './pages/NotFound';

// Central route table. Every entry here is pre-rendered to a real .html file
// at build time, so deep links resolve on any static file server.
//
// Placeholder site: a single landing page plus a catch-all. The full
// multi-page route tree (getting-started, docs/*) will be reintroduced here
// when the real content lands.
export const routes: RouteRecord[] = [
  { path: '/', element: <LandingPage />, entry: 'src/pages/LandingPage.tsx' },
  { path: '*', element: <NotFound />, entry: 'src/pages/NotFound.tsx' },
];
