import type { RouteRecord } from 'vite-react-ssg';
import Layout from './components/Layout';
import LandingPage from './pages/LandingPage';
import GettingStartedPage from './pages/GettingStartedPage';
import DocsConcepts from './pages/docs/DocsConcepts';
import DocsDaemon from './pages/docs/DocsDaemon';
import DocsCbctl from './pages/docs/DocsCbctl';
import DocsClients from './pages/docs/DocsClients';
import DocsProtocol from './pages/docs/DocsProtocol';
import DocsPlatforms from './pages/docs/DocsPlatforms';
import NotFound from './pages/NotFound';

// Central route table. Every entry here is pre-rendered to a real .html file
// at build time, so deep links resolve on any static file server.
export const routes: RouteRecord[] = [
  {
    path: '/',
    element: <Layout />,
    children: [
      { index: true, element: <LandingPage />, entry: 'src/pages/LandingPage.tsx' },
      {
        path: 'getting-started',
        element: <GettingStartedPage />,
        entry: 'src/pages/GettingStartedPage.tsx',
      },
      {
        path: 'docs',
        children: [
          { index: true, element: <DocsConcepts />, entry: 'src/pages/docs/DocsConcepts.tsx' },
          {
            path: 'concepts',
            element: <DocsConcepts />,
            entry: 'src/pages/docs/DocsConcepts.tsx',
          },
          { path: 'daemon', element: <DocsDaemon />, entry: 'src/pages/docs/DocsDaemon.tsx' },
          { path: 'cbctl', element: <DocsCbctl />, entry: 'src/pages/docs/DocsCbctl.tsx' },
          { path: 'clients', element: <DocsClients />, entry: 'src/pages/docs/DocsClients.tsx' },
          {
            path: 'protocol',
            element: <DocsProtocol />,
            entry: 'src/pages/docs/DocsProtocol.tsx',
          },
          {
            path: 'platforms',
            element: <DocsPlatforms />,
            entry: 'src/pages/docs/DocsPlatforms.tsx',
          },
        ],
      },
      { path: '*', element: <NotFound />, entry: 'src/pages/NotFound.tsx' },
    ],
  },
];
