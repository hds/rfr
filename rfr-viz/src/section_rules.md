

| Prev / Current | NewTask | TaskPollStart      | TaskPollEnd          | TaskDrop           | Wake                |
+----------------|---------|--------------------|----------------------|--------------------|---------------------|
| NewTask        | Invalid | idle section       | Invalid              | idle section       | idle section        |
| TaskPollStart  | Invalid | Invalid            | active section       | invalid            | active section      |
| TaskPollEnd    | Invalid | idle section       | Invalid              | idle section       | idle section        |
| TaskDrop       | Invalid | Invalid            | Invalid              | Invalid            | Invalid             |
| Wake           | Invalid | idle-sched section | active-sched section | idle-sched section | Extend last section |
