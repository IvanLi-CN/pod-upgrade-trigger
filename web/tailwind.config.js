import daisyui from 'daisyui'

/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      fontFamily: {
        title: ['"Space Grotesk"', 'Inter', 'system-ui', 'sans-serif'],
      },
    },
  },
  plugins: [daisyui],
  daisyui: {
    themes: [
      {
        vibe: {
          primary: '#4f46e5',
          'primary-content': '#f8fafc',
          secondary: '#0ea5e9',
          accent: '#f97316',
          neutral: '#111827',
          'neutral-content': '#f8fafc',
          'base-100': '#f8fafc',
          'base-200': '#f1f5f9',
          'base-300': '#e2e8f0',
          info: '#0ea5e9',
          success: '#16a34a',
          warning: '#facc15',
          error: '#ef4444',
        },
      },
      'business',
    ],
    darkTheme: 'business',
  },
}
