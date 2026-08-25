import './global.css';

import type { Metadata, Viewport } from 'next';
import type { ReactNode } from 'react';
import { Provider } from './provider';

export const metadata: Metadata = {
  description:
    'Developer documentation for Ktesio, a Rust CLI for installing and sharing reusable agent skills.',
  metadataBase: new URL('https://docs.ktesio.dev'),
  openGraph: {
    description:
      'Developer documentation for Ktesio, a Rust CLI for installing and sharing reusable agent skills.',
    images: ['/assets/ktesio-banner.png'],
    siteName: 'Ktesio Docs',
    title: 'Ktesio Docs',
    type: 'website',
    url: '/',
  },
  title: {
    default: 'Ktesio Docs',
    template: '%s | Ktesio Docs',
  },
};

export const viewport: Viewport = {
  initialScale: 1,
  width: 'device-width',
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body className="flex min-h-screen flex-col">
        <Provider>{children}</Provider>
      </body>
    </html>
  );
}
