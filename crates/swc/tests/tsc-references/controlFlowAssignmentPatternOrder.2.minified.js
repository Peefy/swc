//// [controlFlowAssignmentPatternOrder.ts]
{
    let b, a = 0;
    [{ 1: b  } = [
        9,
        a
    ]] = [];
}{
    let b1, a1 = 1;
    [{ [a1]: b1  } = [
        9,
        0
    ]] = [];
}{
    let b2, a2 = 0;
    [{ 1: b2  } = [
        9,
        a2
    ]] = [
        [
            9,
            8
        ]
    ];
}{
    let b3, a3 = 1;
    [{ [a3]: b3  } = [
        0,
        9
    ]] = [
        [
            8,
            9
        ]
    ];
}{
    let b4, a4 = 0;
    [{ 1: b4  } = [
        9,
        a4
    ]] = [], f();
}{
    let b5, a5 = 1;
    [{ [a5]: b5  } = [
        9,
        0
    ]] = [], f();
}{
    let b6, a6 = 0;
    [{ 1: b6  } = [
        9,
        a6
    ]] = [
        [
            9,
            8
        ]
    ], f();
}{
    let b7, a7 = 1;
    [{ [a7]: b7  } = [
        0,
        9
    ]] = [
        [
            8,
            9
        ]
    ], f();
}{
    let b8, a8 = 0;
    f(), [{ 1: b8  } = [
        9,
        a8
    ]] = [];
}{
    let b9, a9 = 1;
    f(), [{ [a9]: b9  } = [
        9,
        0
    ]] = [];
}{
    let b10, a10 = 0;
    f(), [{ 1: b10  } = [
        9,
        a10
    ]] = [
        [
            9,
            8
        ]
    ];
}{
    let b11, a11 = 1;
    f(), [{ [a11]: b11  } = [
        0,
        9
    ]] = [
        [
            8,
            9
        ]
    ];
}
