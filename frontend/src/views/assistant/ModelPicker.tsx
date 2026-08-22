import { useMemo, useRef, useState } from "react";

interface Props {
  id: string;
  value: string;
  models: string[];
  onChange: (modelId: string) => void;
}

export function ModelPicker({ id, value, models, onChange }: Props) {
  const inputRef = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const listId = `${id}-list`;
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const filteredModels = useMemo(
    () =>
      normalizedQuery
        ? models.filter((model) =>
            model.toLocaleLowerCase().includes(normalizedQuery),
          )
        : models,
    [models, normalizedQuery],
  );
  const selectedAvailable = models.includes(value);
  const disabled = models.length === 0;

  function openMenu() {
    if (disabled) return;
    setQuery("");
    setActiveIndex(Math.max(0, models.indexOf(value)));
    setOpen(true);
  }

  function selectModel(modelId: string) {
    onChange(modelId);
    setQuery("");
    setOpen(false);
    inputRef.current?.focus();
  }

  function handleKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Escape") {
      setOpen(false);
      setQuery("");
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!open) {
        openMenu();
        return;
      }
      if (filteredModels.length === 0) return;
      const direction = event.key === "ArrowDown" ? 1 : -1;
      setActiveIndex((current) =>
        (current + direction + filteredModels.length) % filteredModels.length,
      );
      return;
    }
    if (event.key === "Enter" && open && filteredModels[activeIndex]) {
      event.preventDefault();
      selectModel(filteredModels[activeIndex]);
    }
  }

  return (
    <div
      className="assistant-model-picker"
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) {
          setOpen(false);
          setQuery("");
        }
      }}
    >
      <div className="assistant-model-picker-control">
        <input
          ref={inputRef}
          id={id}
          aria-label="Model"
          role="combobox"
          aria-autocomplete="list"
          aria-controls={listId}
          aria-expanded={open}
          aria-activedescendant={
            open && filteredModels[activeIndex]
              ? `${id}-option-${activeIndex}`
              : undefined
          }
          aria-invalid={value !== "" && !selectedAvailable}
          disabled={disabled}
          value={open ? query : value}
          maxLength={256}
          placeholder={
            disabled
              ? "Verify a connection to load its models"
              : open
                ? "Filter available models"
                : "Choose a verified model"
          }
          autoComplete="off"
          onFocus={openMenu}
          onChange={(event) => {
            setQuery(event.target.value);
            setActiveIndex(0);
          }}
          onKeyDown={handleKeyDown}
        />
        <button
          type="button"
          className="assistant-model-picker-toggle"
          aria-label={open ? "Close available models" : "Show available models"}
          aria-expanded={open}
          aria-controls={listId}
          disabled={disabled}
          onClick={() => {
            if (open) {
              setOpen(false);
              setQuery("");
            } else {
              inputRef.current?.focus();
              openMenu();
            }
          }}
        >
          <span aria-hidden="true">⌄</span>
        </button>
        {open ? (
          <div id={listId} className="assistant-model-picker-menu" role="listbox">
            <div className="assistant-model-picker-count">
              {filteredModels.length === models.length
                ? `${models.length} available models`
                : `${filteredModels.length} of ${models.length} models`}
            </div>
            {filteredModels.length > 0 ? (
              <div className="assistant-model-picker-options">
                {filteredModels.map((model, index) => (
                  <button
                    id={`${id}-option-${index}`}
                    key={model}
                    type="button"
                    role="option"
                    tabIndex={-1}
                    aria-selected={model === value}
                    className={index === activeIndex ? "is-active" : undefined}
                    onMouseDown={(event) => event.preventDefault()}
                    onMouseEnter={() => setActiveIndex(index)}
                    onClick={() => selectModel(model)}
                  >
                    {model}
                  </button>
                ))}
              </div>
            ) : (
              <p className="assistant-model-picker-empty">
                No verified model matches “{query}”.
              </p>
            )}
          </div>
        ) : null}
      </div>

    </div>
  );
}
