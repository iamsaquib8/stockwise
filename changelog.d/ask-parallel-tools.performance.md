- `ask`: when the model asks for several tools in one turn, up to 4 of them now run at the same time
  instead of one after another, and the results still go back in the model's order. The per-turn tool-call
  limit counts each call as before ([#113](https://github.com/iamsaquib8/stockwise/issues/113)).
