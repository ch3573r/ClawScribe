/** @type {import('tailwindcss').Config} */
const hsl = (variable) => `hsl(var(${variable}) / <alpha-value>)`;

module.exports = {
    darkMode: ['class'],
    content: [
    './src/pages/**/*.{js,ts,jsx,tsx,mdx}',
    './src/components/**/*.{js,ts,jsx,tsx,mdx}',
    './src/app/**/*.{js,ts,jsx,tsx,mdx}',
  ],
  theme: {
    extend: {
      fontFamily: {
        sans: [
          'var(--font-source-sans-3)'
        ],
        mono: [
          'var(--font-plex-mono)',
          'ui-monospace',
          'SFMono-Regular',
          'Menlo',
          'monospace'
        ]
      },
      colors: {
        background: hsl('--background'),
        foreground: hsl('--foreground'),
        border: hsl('--border'),
        input: hsl('--input'),
        ring: hsl('--ring'),
        primary: {
          DEFAULT: hsl('--primary'),
          foreground: hsl('--primary-foreground')
        },
        secondary: {
          DEFAULT: hsl('--secondary'),
          foreground: hsl('--secondary-foreground')
        },
        tertiary: hsl('--theme-slate-600'),
        card: {
          DEFAULT: hsl('--card'),
          foreground: hsl('--card-foreground')
        },
        popover: {
          DEFAULT: hsl('--popover'),
          foreground: hsl('--popover-foreground')
        },
        muted: {
          DEFAULT: hsl('--muted'),
          foreground: hsl('--muted-foreground')
        },
        accent: {
          DEFAULT: hsl('--accent'),
          foreground: hsl('--accent-foreground')
        },
        sidebar: {
          DEFAULT: hsl('--sidebar'),
          foreground: hsl('--sidebar-foreground'),
          border: hsl('--sidebar-border'),
          hover: hsl('--sidebar-hover'),
          active: hsl('--sidebar-active'),
          'active-foreground': hsl('--sidebar-active-foreground')
        },
        destructive: {
          DEFAULT: hsl('--destructive'),
          foreground: hsl('--destructive-foreground')
        },
        chart: {
          '1': hsl('--chart-1'),
          '2': hsl('--chart-2'),
          '3': hsl('--chart-3'),
          '4': hsl('--chart-4'),
          '5': hsl('--chart-5')
        },
        blue: {
          50: hsl('--theme-blue-50'),
          100: hsl('--theme-blue-100'),
          200: hsl('--theme-blue-100'),
          300: hsl('--kontron-light-blue'),
          400: hsl('--kontron-mid-blue'),
          500: hsl('--theme-blue-500'),
          600: hsl('--theme-blue-600'),
          700: hsl('--theme-blue-700'),
          800: hsl('--kontron-primary'),
          900: hsl('--kontron-black-blue'),
        },
        gray: {
          50: hsl('--theme-gray-50'),
          100: hsl('--theme-gray-100'),
          200: hsl('--theme-gray-200'),
          300: hsl('--theme-gray-300'),
          400: hsl('--kontron-grey'),
          500: hsl('--theme-gray-500'),
          600: hsl('--theme-gray-600'),
          700: hsl('--theme-gray-700'),
          800: hsl('--theme-gray-800'),
          900: hsl('--theme-gray-900'),
        },
        slate: {
          50: hsl('--theme-slate-50'),
          100: hsl('--theme-slate-100'),
          600: hsl('--theme-slate-600'),
          700: hsl('--theme-slate-700'),
        },
        green: {
          100: hsl('--theme-success-bg'),
          200: hsl('--theme-success-bg'),
          500: hsl('--kontron-accent'),
          600: hsl('--kontron-accent'),
          700: hsl('--theme-success-fg'),
          800: hsl('--theme-success-fg'),
        },
        amber: {
          100: hsl('--theme-warning-bg'),
          700: hsl('--theme-warning-fg'),
          800: hsl('--theme-warning-fg'),
        },
        yellow: {
          50: hsl('--theme-warning-bg'),
          100: hsl('--theme-warning-bg'),
          600: hsl('--theme-warning-fg'),
          700: hsl('--theme-warning-fg'),
          800: hsl('--theme-warning-fg'),
        },
      },
      borderRadius: {
        lg: 'var(--radius)',
        md: 'calc(var(--radius) - 2px)',
        sm: 'calc(var(--radius) - 4px)'
      },
      keyframes: {
        'accordion-down': {
          from: {
            height: '0'
          },
          to: {
            height: 'var(--radix-accordion-content-height)'
          }
        },
        'accordion-up': {
          from: {
            height: 'var(--radix-accordion-content-height)'
          },
          to: {
            height: '0'
          }
        }
      },
      animation: {
        'accordion-down': 'accordion-down 0.2s ease-out',
        'accordion-up': 'accordion-up 0.2s ease-out'
      }
    }
  },
  plugins: [require("tailwindcss-animate")],
}
