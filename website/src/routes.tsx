import type { RouteRecord } from 'vite-react-ssg';
import Layout from './components/Layout';
import LandingPage from './pages/LandingPage';
import GettingStartedPage from './pages/GettingStartedPage';
import DocsConcepts from './pages/docs/DocsConcepts';
import DocsDaemon from './pages/docs/DocsDaemon';
import DocsCbctl from './pages/docs/DocsCbctl';
import DocsCbctlRedirect from './pages/docs/DocsCbctlRedirect';
import DocsDaemonConfiguration from './pages/docs/DocsDaemonConfiguration';
import DocsDaemonLogging from './pages/docs/DocsDaemonLogging';
import DocsLogSynchronization from './pages/docs/DocsLogSynchronization';
import DocsLogFFEvents from './pages/docs/DocsLogFFEvents';
import DocsLogSources from './pages/docs/DocsLogSources';
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
          {
            path: 'daemon/configuration',
            element: <DocsDaemonConfiguration />,
            entry: 'src/pages/docs/DocsDaemonConfiguration.tsx',
          },
          {
            path: 'daemon/logging',
            element: <DocsDaemonLogging />,
            entry: 'src/pages/docs/DocsDaemonLogging.tsx',
          },
          {
            path: 'daemon/logging/synchronization',
            element: <DocsLogSynchronization />,
            entry: 'src/pages/docs/DocsLogSynchronization.tsx',
          },
          {
            path: 'daemon/logging/ffevents',
            element: <DocsLogFFEvents />,
            entry: 'src/pages/docs/DocsLogFFEvents.tsx',
          },
          {
            path: 'daemon/logging/sources',
            element: <DocsLogSources />,
            entry: 'src/pages/docs/DocsLogSources.tsx',
          },
          {
            path: 'daemon/cbctl',
            element: <DocsCbctl />,
            entry: 'src/pages/docs/DocsCbctl.tsx',
          },
          // Moved to daemon/cbctl; kept so existing links keep working.
          {
            path: 'cbctl',
            element: <DocsCbctlRedirect />,
            entry: 'src/pages/docs/DocsCbctlRedirect.tsx',
          },
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
