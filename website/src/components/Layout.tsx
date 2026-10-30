import { Outlet, useLocation } from 'react-router-dom';
import { useEffect } from 'react';
import NavBar from './NavBar';
import Footer from './Footer';

// App shell: persistent nav + footer with the routed page in between.
export default function Layout() {
  const { pathname, hash } = useLocation();

  // On route change, scroll to the section named by the URL hash (deep links
  // like /docs/daemon#logging), or to the top of the page when there is none.
  // Without the hash check, the scroll-to-top would undo the browser's own
  // jump to the section on page load.
  useEffect(() => {
    if (hash) {
      const el = document.getElementById(decodeURIComponent(hash.slice(1)));
      if (el) {
        el.scrollIntoView();
        return;
      }
    }
    window.scrollTo(0, 0);
  }, [pathname, hash]);

  return (
    <div className="flex min-h-screen flex-col">
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:absolute focus:left-4 focus:top-4 focus:z-[60] focus:rounded focus:bg-accent focus:px-4 focus:py-2 focus:text-white"
      >
        Skip to content
      </a>
      <NavBar />
      <main id="main" className="flex-1">
        <Outlet />
      </main>
      <Footer />
    </div>
  );
}
