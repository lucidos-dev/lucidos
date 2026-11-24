interface ListRowAddCardProps {
  label: string;
  onClick: () => void;
}

export function ListRowAddCard({ label, onClick }: ListRowAddCardProps) {
  return (
    <button type="button" class="list-row-add-card" onClick={onClick}>
      <span class="list-row-add-icon">+</span>
      <span class="list-row-add-label">{label}</span>
    </button>
  );
}
