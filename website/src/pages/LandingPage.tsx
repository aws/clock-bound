// Placeholder landing page for the ClockBound v3 site.
//
// Self-contained on purpose: it pulls in none of the full site's design-system
// components (NavBar, Footer, Button, widgets) so the placeholder can build and
// deploy on its own. When the real content lands, this file is replaced and the
// route tree in routes.tsx is expanded.
export default function LandingPage() {
  return (
    <main className="container-page flex min-h-screen flex-col items-center justify-center text-center">
      <div className="animate-fade-in-up">
        <p className="font-mono text-sm uppercase tracking-[0.3em] text-accent-hover">
          ClockBound
        </p>

        <h1 className="mt-6 bg-accent-gradient bg-clip-text text-4xl font-extrabold leading-tight text-transparent sm:text-6xl">
          ClockBound v3 website coming soon&hellip;
        </h1>

        <p className="mx-auto mt-6 max-w-xl text-lg text-slate-400">
          Bounded, trustworthy time for distributed systems. The full site is on
          its way.
        </p>

        <div className="mt-10">
          <a
            href="https://github.com/aws/clock-bound"
            className="inline-flex items-center justify-center rounded-xl bg-accent px-6 py-3 font-medium text-white shadow-glow transition-colors hover:bg-accent-hover"
          >
            View on GitHub
          </a>
        </div>
      </div>
    </main>
  );
}
