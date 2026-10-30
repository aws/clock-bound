/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{js,ts,jsx,tsx}'],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        // Deep slate/black canvas
        base: {
          900: '#0b0f19',
          800: '#111726',
          700: '#1a2234',
          600: '#232d44',
        },
        // Primary accent: electric indigo
        accent: {
          DEFAULT: '#6366f1',
          hover: '#818cf8',
          soft: '#4f46e5',
        },
        // Secondary accent: neon cyan (used sparingly for diagrams/highlights)
        cyan: {
          glow: '#22d3ee',
        },
        // Tertiary accent: emerald (success / "synchronized" states)
        emerald: {
          glow: '#34d399',
        },
      },
      fontFamily: {
        sans: ['Inter', 'ui-sans-serif', 'system-ui', 'sans-serif'],
        mono: ['"JetBrains Mono"', 'ui-monospace', 'SFMono-Regular', 'monospace'],
      },
      backgroundImage: {
        'grid-slate':
          'linear-gradient(to right, rgba(148,163,184,0.06) 1px, transparent 1px), linear-gradient(to bottom, rgba(148,163,184,0.06) 1px, transparent 1px)',
        'accent-gradient': 'linear-gradient(135deg, #6366f1 0%, #22d3ee 100%)',
      },
      boxShadow: {
        glow: '0 0 40px -10px rgba(99,102,241,0.5)',
        'glow-cyan': '0 0 40px -10px rgba(34,211,238,0.45)',
      },
      keyframes: {
        'fade-in-up': {
          '0%': { opacity: '0', transform: 'translateY(12px)' },
          '100%': { opacity: '1', transform: 'translateY(0)' },
        },
      },
      animation: {
        'fade-in-up': 'fade-in-up 0.6s ease-out both',
      },
    },
  },
  plugins: [],
};
