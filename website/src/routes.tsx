import type { RouteRecord } from 'vite-react-ssg';
import { Outlet } from 'react-router-dom';
import LandingPage from './pages/LandingPage';
import NotFound from './pages/NotFound';

// Minimal app shell. The placeholder has no NavBar/Footer yet, so the layout
// is just the routed <Outlet />. Keeping a parent route with an `index` child
// (rather than a bare top-level `path: '/'`) matches the structure the full
// site uses and is what vite-react-ssg + the GitHub Pages basename expect:
// the index route resolves the site root, and the `*` catch-all is nested so
// it only handles genuinely unknown sub-paths instead of competing with root.
function Layout() {
  return <Outlet />;
}

// Central route table. Every entry here is pre-rendered to a real .html file
// at build time, so deep links resolve on any static file server.
export const routes: RouteRecord[] = [
  {
    path: '/',
    element: <Layout />,
    children: [
      { index: true, element: <LandingPage />, entry: 'src/pages/LandingPage.tsx' },
      { path: '*', element: <NotFound />, entry: 'src/pages/NotFound.tsx' },
    ],
  },
];
