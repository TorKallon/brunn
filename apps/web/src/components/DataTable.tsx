import {
  flexRender,
  getCoreRowModel,
  useReactTable,
  type ColumnDef,
} from "@tanstack/react-table";
import { ChevronRight } from "lucide-react";
import type { ReactNode } from "react";
import { EmptyState } from "./StateViews";

export function DataTable<T>({
  data,
  columns,
  emptyTitle = "No records",
  onRowClick,
  getRowLabel,
}: {
  data: T[];
  columns: ColumnDef<T, unknown>[];
  emptyTitle?: string;
  onRowClick?: (row: T) => void;
  getRowLabel?: (row: T) => string;
}) {
  const table = useReactTable({ data, columns, getCoreRowModel: getCoreRowModel() });
  if (!data.length) return <EmptyState title={emptyTitle} />;

  return (
    <div className="table-wrap">
      <table className="data-table">
        <thead>
          {table.getHeaderGroups().map((headerGroup) => (
            <tr key={headerGroup.id}>
              {headerGroup.headers.map((header) => (
                <th key={header.id} colSpan={header.colSpan}>
                  {header.isPlaceholder
                    ? null
                    : flexRender(header.column.columnDef.header, header.getContext())}
                </th>
              ))}
              {onRowClick ? <th className="row-action-cell"><span className="sr-only">Open</span></th> : null}
            </tr>
          ))}
        </thead>
        <tbody>
          {table.getRowModel().rows.map((row) => (
            <tr
              key={row.id}
              className={onRowClick ? "clickable-row" : undefined}
              onClick={onRowClick ? () => onRowClick(row.original) : undefined}
              onKeyDown={
                onRowClick
                  ? (event) => {
                      if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault();
                        onRowClick(row.original);
                      }
                    }
                  : undefined
              }
              tabIndex={onRowClick ? 0 : undefined}
              aria-label={getRowLabel?.(row.original)}
            >
              {row.getVisibleCells().map((cell) => (
                <td
                  key={cell.id}
                  data-label={
                    typeof cell.column.columnDef.header === "string"
                      ? cell.column.columnDef.header
                      : undefined
                  }
                >
                  {flexRender(cell.column.columnDef.cell, cell.getContext()) as ReactNode}
                </td>
              ))}
              {onRowClick ? (
                <td className="row-action-cell">
                  <ChevronRight size={16} aria-hidden="true" />
                </td>
              ) : null}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
