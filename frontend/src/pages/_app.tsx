import type { AppProps } from 'next/app';
import '@/app/globals.css';

// Pages Router windows have their own entry point, outside the desktop shell.
export default function WindowApp({ Component, pageProps }: AppProps) {
  return <Component {...pageProps} />;
}
