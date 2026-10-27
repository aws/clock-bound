export default function NotFound() {
  return (
    <div className="container-page flex min-h-[60vh] flex-col items-center justify-center text-center">
      <p className="font-mono text-6xl font-bold text-accent">404</p>
      <h1 className="mt-4 text-2xl font-bold text-white">Page not found</h1>
      <p className="mt-2 max-w-md text-slate-400">
        That page drifted outside the error bound. Let&apos;s get you back to
        synchronized ground.
      </p>
      <div className="mt-6">
        <a
          href="/"
          className="inline-flex items-center justify-center rounded-xl bg-accent px-6 py-3 font-medium text-white shadow-glow transition-colors hover:bg-accent-hover"
        >
          Back to home
        </a>
      </div>
    </div>
  );
}
