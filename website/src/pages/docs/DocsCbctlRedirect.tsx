import { useEffect } from 'react';
import { Link, useLocation, useNavigate } from 'react-router-dom';

// The cbctl reference moved from /docs/cbctl to /docs/daemon/cbctl. The site
// is static with no server-side rewrites, so this pre-rendered page keeps old
// links working: a meta refresh redirects without JavaScript, and after
// hydration the router redirects in place, keeping any #section fragment.
const TARGET = '/docs/daemon/cbctl';

export default function DocsCbctlRedirect() {
  const navigate = useNavigate();
  const { hash } = useLocation();

  useEffect(() => {
    navigate(`${TARGET}${hash}`, { replace: true });
  }, [navigate, hash]);

  return (
    <div className="container-page flex min-h-[60vh] flex-col items-center justify-center text-center">
      <meta httpEquiv="refresh" content={`0; url=${import.meta.env.BASE_URL}${TARGET.slice(1)}`} />
      <h1 className="text-2xl font-bold text-white">This page has moved</h1>
      <p className="mt-2 text-slate-400">
        The cbctl reference is now at{' '}
        <Link to={TARGET} className="text-accent-hover underline underline-offset-2 hover:text-white">
          Daemon › cbctl
        </Link>
        .
      </p>
    </div>
  );
}
