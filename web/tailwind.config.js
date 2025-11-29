import daisyui from 'daisyui'
import { components } from 'daisyui/imports.js'

// Ensure tab styles are always generated even if Tree-shake misses them
const daisyComponents = Object.keys(components)
if (!daisyComponents.includes('tab')) {
  daisyComponents.push('tab')
}

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
    // Explicitly include all component modules so styled components (like tabs)
    // are guaranteed to be generated.
    include: daisyComponents,
  },
}
