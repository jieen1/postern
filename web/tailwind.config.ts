import type { Config } from 'tailwindcss';

// Design tokens (设计系统 §3) are declared as CSS variables in
// src/styles/tokens.css and merely *mapped* here, so Tailwind utilities and
// raw CSS share one source of truth and dark/light theming is a single
// [data-theme] swap with no utility regeneration.
const config: Config = {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  darkMode: ['class', '[data-theme="dark"]'],
  theme: {
    extend: {
      colors: {
        bg: 'var(--bg)',
        surface: 'var(--surface)',
        'surface-2': 'var(--surface-2)',
        border: 'var(--border)',
        text: 'var(--text)',
        'text-muted': 'var(--text-muted)',
        // Semantic status (§3.1) — fixed meaning, never decorative.
        allow: 'var(--allow)',
        deny: 'var(--deny)',
        warn: 'var(--warn)',
        info: 'var(--info)',
        freeze: 'var(--freeze)',
        // Capability verb colors (§3.1) — danger ascends with color temp.
        'cap-observe': 'var(--cap-observe)',
        'cap-query': 'var(--cap-query)',
        'cap-mutate': 'var(--cap-mutate)',
        'cap-execute': 'var(--cap-execute)',
        'cap-manage': 'var(--cap-manage)',
        'cap-destroy': 'var(--cap-destroy)',
        // Mode colors (§3.1).
        'mode-normal': 'var(--mode-normal)',
        'mode-observe': 'var(--mode-observe)',
        'mode-maintain': 'var(--mode-maintain)',
        'mode-freeze': 'var(--mode-freeze)',
      },
      borderColor: {
        DEFAULT: 'var(--border)',
      },
      borderRadius: {
        // §3.3 — 6 for cards/inputs, 4 for badges.
        card: 'var(--radius-card)',
        badge: 'var(--radius-badge)',
      },
      spacing: {
        // §3.3 — 4-based scale.
        1: 'var(--space-1)',
        2: 'var(--space-2)',
        3: 'var(--space-3)',
        4: 'var(--space-4)',
        6: 'var(--space-6)',
        8: 'var(--space-8)',
      },
      fontFamily: {
        sans: 'var(--font-sans)',
        mono: 'var(--font-mono)',
      },
      fontSize: {
        // §3.2 — 12/13/14/16/20/24, table density at 13.
        xs: ['12px', { lineHeight: '1.4' }],
        sm: ['13px', { lineHeight: '1.4' }],
        base: ['14px', { lineHeight: '1.5' }],
        lg: ['16px', { lineHeight: '1.5' }],
        xl: ['20px', { lineHeight: '1.4' }],
        '2xl': ['24px', { lineHeight: '1.3' }],
      },
      keyframes: {
        'freeze-pulse': {
          '0%, 100%': { opacity: '1' },
          '50%': { opacity: '0.55' },
        },
      },
      animation: {
        'freeze-pulse': 'freeze-pulse 1.6s ease-in-out infinite',
      },
    },
  },
  plugins: [],
};

export default config;
