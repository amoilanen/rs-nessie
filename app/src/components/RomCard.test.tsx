// Unit tests for the `RomCard` component.

import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { RomEntry } from '../ipc/types';

import { RomCard } from './RomCard';

const sampleRom: RomEntry = {
  id: '11111111-1111-4111-8111-111111111111',
  title: 'Test ROM',
  path: '/tmp/test.nes',
  sha1: 'a'.repeat(40),
  mapper: 4,
  size_bytes: 262_144,
  imported_at: 1_700_000_000_000,
};

describe('RomCard', () => {
  it('renders the ROM title and mapper number', () => {
    render(<RomCard rom={sampleRom} onPlay={(): void => {}} />);
    expect(screen.getByText('Test ROM')).toBeInTheDocument();
    // Mapper value is rendered in a definition list under the "Mapper" term.
    expect(screen.getByText('Mapper')).toBeInTheDocument();
    expect(screen.getByText('4')).toBeInTheDocument();
  });

  it('renders the size in KB', () => {
    render(<RomCard rom={sampleRom} onPlay={(): void => {}} />);
    // 262_144 bytes = 256 KB.
    expect(screen.getByText('256 KB')).toBeInTheDocument();
  });

  it('invokes onPlay with the ROM id when the Play button is clicked', () => {
    const onPlay = vi.fn();
    render(<RomCard rom={sampleRom} onPlay={onPlay} />);
    fireEvent.click(screen.getByRole('button', { name: 'Play' }));
    expect(onPlay).toHaveBeenCalledTimes(1);
    expect(onPlay).toHaveBeenCalledWith(sampleRom.id);
  });

  it('renders optional secondary actions only when handlers are provided', () => {
    const onPlay = vi.fn();
    const onRename = vi.fn();
    const onRemove = vi.fn();
    const { rerender } = render(<RomCard rom={sampleRom} onPlay={onPlay} />);
    expect(screen.queryByRole('button', { name: 'Rename' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Remove' })).toBeNull();

    rerender(
      <RomCard
        rom={sampleRom}
        onPlay={onPlay}
        onRename={onRename}
        onRemove={onRemove}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Rename' }));
    fireEvent.click(screen.getByRole('button', { name: 'Remove' }));
    expect(onRename).toHaveBeenCalledWith(sampleRom.id);
    expect(onRemove).toHaveBeenCalledWith(sampleRom.id);
  });
});
