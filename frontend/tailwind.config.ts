import type { Config } from "tailwindcss";

const hsl = (variable: string) => `hsl(var(${variable}) / <alpha-value>)`;

export default {
  darkMode: "class",
  content: [
    "./src/pages/**/*.{js,ts,jsx,tsx,mdx}",
    "./src/components/**/*.{js,ts,jsx,tsx,mdx}",
    "./src/app/**/*.{js,ts,jsx,tsx,mdx}",
  ],
  theme: {
    extend: {
      colors: {
        background: hsl("--background"),
        foreground: hsl("--foreground"),
        card: {
          DEFAULT: hsl("--card"),
          foreground: hsl("--card-foreground"),
        },
        popover: {
          DEFAULT: hsl("--popover"),
          foreground: hsl("--popover-foreground"),
        },
        primary: {
          DEFAULT: hsl("--primary"),
          foreground: hsl("--primary-foreground"),
        },
        secondary: {
          DEFAULT: hsl("--secondary"),
          foreground: hsl("--secondary-foreground"),
        },
        muted: {
          DEFAULT: hsl("--muted"),
          foreground: hsl("--muted-foreground"),
        },
        accent: {
          DEFAULT: hsl("--accent"),
          foreground: hsl("--accent-foreground"),
        },
        destructive: {
          DEFAULT: hsl("--destructive"),
          foreground: hsl("--destructive-foreground"),
        },
        border: hsl("--border"),
        input: hsl("--input"),
        ring: hsl("--ring"),
        blue: {
          50: hsl("--theme-blue-50"),
          100: hsl("--theme-blue-100"),
          200: hsl("--theme-blue-100"),
          300: hsl("--kontron-light-blue"),
          400: hsl("--kontron-mid-blue"),
          500: hsl("--theme-blue-500"),
          600: hsl("--theme-blue-600"),
          700: hsl("--theme-blue-700"),
          800: hsl("--kontron-primary"),
          900: hsl("--kontron-black-blue"),
        },
        gray: {
          50: hsl("--theme-gray-50"),
          100: hsl("--theme-gray-100"),
          200: hsl("--theme-gray-200"),
          300: hsl("--theme-gray-300"),
          400: hsl("--kontron-grey"),
          500: hsl("--theme-gray-500"),
          600: hsl("--theme-gray-600"),
          700: hsl("--theme-gray-700"),
          800: hsl("--theme-gray-800"),
          900: hsl("--theme-gray-900"),
        },
        slate: {
          50: hsl("--theme-slate-50"),
          100: hsl("--theme-slate-100"),
          600: hsl("--theme-slate-600"),
          700: hsl("--theme-slate-700"),
        },
        green: {
          100: hsl("--theme-success-bg"),
          200: hsl("--theme-success-bg"),
          500: hsl("--kontron-accent"),
          600: hsl("--kontron-accent"),
          700: hsl("--theme-success-fg"),
          800: hsl("--theme-success-fg"),
        },
        amber: {
          100: hsl("--theme-warning-bg"),
          700: hsl("--theme-warning-fg"),
          800: hsl("--theme-warning-fg"),
        },
        yellow: {
          50: hsl("--theme-warning-bg"),
          100: hsl("--theme-warning-bg"),
          600: hsl("--theme-warning-fg"),
          700: hsl("--theme-warning-fg"),
          800: hsl("--theme-warning-fg"),
        },
      },
      fontSize: {
        'display': ['32px', { lineHeight: '1.2', fontWeight: '700' }],
        'h1': ['24px', { lineHeight: '1.3', fontWeight: '600' }],
        'h2': ['18px', { lineHeight: '1.4', fontWeight: '500' }],
        'body': ['16px', { lineHeight: '1.6', fontWeight: '400' }],
        'small': ['14px', { lineHeight: '1.5', fontWeight: '400' }],
        'caption': ['12px', { lineHeight: '1.4', fontWeight: '400' }],
      },
      borderRadius: {
        DEFAULT: '3px',
        sm: '2px',
        md: '3px',
        lg: 'var(--radius)',
        xl: 'calc(var(--radius) + 2px)',
        '2xl': 'calc(var(--radius) + 4px)',
      },
    },
  },
  plugins: [
    require('@tailwindcss/typography'),
  ],
} satisfies Config;
